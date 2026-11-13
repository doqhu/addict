#!/usr/bin/env python3
"""Verify production capture with known Screenshot responses on a disposable X11 desktop.

Only Screenshot and its required app-id Registry are mocked. Native Capture is
invoked through AT-SPI; the actual GTK field is read and asserted there.
No OCR helper, user OCR override, host engine or host model participates.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

TEXTS = {'eng': ('en', 'The bank is beside the river.'),
         'spa': ('es', 'El niño está cerca del río.'),
         'cat': ('ca', 'El camí és a prop del riu.')}
ROOT = Path(__file__).resolve().parents[2]
DESKTOP = '/org/freedesktop/portal/desktop'


def run(args, **kwargs):
    return subprocess.run(args, check=True, timeout=15, capture_output=True, **kwargs)


def outer(image, out):
    if not os.environ.get('DISPLAY'):
        raise SystemExit('an X11 desktop DISPLAY is required (Xwayland on GNOME is supported)')
    if Path('/dev/fuse').exists():
        raise SystemExit('verification requires a disposable machine without /dev/fuse')
    if shutil.which('tesseract'):
        raise SystemExit('remove host tesseract before verification')
    for pattern in ['/usr/share/tessdata', '/usr/share/tesseract-ocr',
                    '/usr/local/share/tessdata', '/usr/local/share/tesseract-ocr']:
        if Path(pattern).exists(): raise SystemExit('remove host model directory: '+pattern)
    for module in ['gi', 'pyatspi']:
        if importlib.util.find_spec(module) is None:
            raise SystemExit('missing verification Python module: '+module)
    for tool in ['strace', 'dbus-run-session']:
        if not shutil.which(tool): raise SystemExit('missing verification tool: '+tool)
    # Validation always precedes the evidence directory and any success record.
    run([str(ROOT/'scripts/check-appimage.sh'), str(image)])
    out.mkdir(parents=True, exist_ok=False)
    (out/'host.txt').write_text(Path('/etc/os-release').read_text()+'\n'+os.uname().release+
                              '\nDISPLAY='+os.environ['DISPLAY']+'\nno host OCR, models or /dev/fuse\n')
    (out/'image.sha256').write_text(hashlib.file_digest(image.open('rb'), 'sha256').hexdigest()+'\n')
    services = out/'services'; services.mkdir()
    shutil.copy2('/usr/share/dbus-1/services/org.a11y.Bus.service', services)
    bus = out/'bus.conf'
    bus.write_text('<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>'
                   '<servicedir>'+str(services)+'</servicedir><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/>'
                   '<allow receive_sender="*"/><allow own="*"/></policy></busconfig>')
    for lang, (reading, _) in TEXTS.items():
        profile = out/lang
        profile.mkdir()
        env = os.environ.copy()
        for key in list(env):
            if key.startswith(('APPIMAGE', 'LD_', 'TESS', 'SHARUN', 'GDK_', 'GTK_', 'GIO_', 'GI_', 'AT_SPI_', 'ATSPI_')) or key in [
                    'APPDIR', 'ARG0', 'ARGV0', 'TARGET_APPIMAGE', 'DBUS_SESSION_BUS_ADDRESS', 'NO_AT_BRIDGE']:
                env.pop(key, None)
        for key, name in [('HOME','home'), ('XDG_DATA_HOME','data'), ('XDG_CONFIG_HOME','config'),
                          ('XDG_CACHE_HOME','cache'), ('XDG_RUNTIME_DIR','runtime')]:
            path = profile/name; path.mkdir(mode=0o700); env[key] = str(path)
        config = profile/'config/addict'; config.mkdir()
        (config/'language').write_text(reading+'\n')
        (config/'first-run-download').write_text('declined\n')
        env.update(PATH='/usr/bin:/bin', GDK_BACKEND='x11',
                   XAUTHORITY=os.environ.get('XAUTHORITY', str(Path.home()/'.Xauthority')))
        env.pop('WAYLAND_DISPLAY', None)
        run_args = ['dbus-run-session', '--config-file', str(bus), '--', sys.executable,
                    str(Path(__file__).resolve()), '--inside', str(image), str(profile), lang]
        with (profile/'harness.log').open('w') as log:
            subprocess.run(run_args, env=env, stdout=log, stderr=log, timeout=240, check=True)
    # Written only after all languages and their bounded cleanup pass.
    (out/'PASS.json').write_text(json.dumps({lang: text for lang, (_, text) in TEXTS.items()}, indent=2)+'\n')
    print('PASS: production EN/ES/CA capture, bundled child/libraries/models and cleanup; '+str(out))


def inside(image, out, lang):
    import gi
    gi.require_version('Gio', '2.0')
    from gi.repository import Gio, GLib
    connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    # libatspi can cache an empty desktop if initialized before the private
    # accessibility bus/registry exists. Activate both and query the exported
    # root before importing pyatspi; never inherit the real desktop's address.
    address = connection.call_sync(
        'org.a11y.Bus', '/org/a11y/bus', 'org.a11y.Bus', 'GetAddress', None,
        GLib.VariantType.new('(s)'), Gio.DBusCallFlags.NONE, 5000, None).unpack()[0]
    accessibility = Gio.DBusConnection.new_for_address_sync(
        address, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT |
        Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)
    accessibility.call_sync(
        'org.a11y.atspi.Registry', '/org/a11y/atspi/accessible/root',
        'org.a11y.atspi.Accessible', 'GetChildren', None, None,
        Gio.DBusCallFlags.NONE, 5000, None)
    os.environ['AT_SPI_BUS_ADDRESS'] = address
    import pyatspi
    print('private accessibility bus and registry ready', flush=True)
    connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
                         'RequestName', GLib.Variant('(su)', ('org.freedesktop.portal.Desktop', 0)),
                         None, Gio.DBusCallFlags.NONE, 5000, None)
    xml = '''<node><interface name="org.freedesktop.host.portal.Registry"><method name="Register">
    <arg type="s" direction="in"/><arg type="a{sv}" direction="in"/></method></interface>
    <interface name="org.freedesktop.portal.Screenshot"><property name="version" type="u" access="read"/>
    <method name="Screenshot"><arg type="s" direction="in"/><arg type="a{sv}" direction="in"/>
    <arg type="o" direction="out"/></method></interface></node>'''
    registered, shots, observed = set(), [], {}
    bundle = resident = None

    def method(conn, sender, path, interface, name, params, invocation):
        if name == 'Register':
            assert params.unpack()[0] == 'io.github.doqhu.Addict'
            registered.add(sender); invocation.return_value(GLib.Variant('()', ())); return
        assert sender in registered
        options = params.unpack()[1]
        assert options['interactive'] is True
        assert not shots, 'exactly one production capture is expected'
        snapshot = out/'region.png'
        shutil.copy2(Path(__file__).parent/'fixtures'/(lang+'.png'), snapshot)
        shots.append({'language': lang, 'fixture_sha256': hashlib.sha256(snapshot.read_bytes()).hexdigest(),
                      'uri': snapshot.as_uri(), 'options': options})
        request = DESKTOP+'/request/'+sender.lstrip(':').replace('.', '_')+'/'+options['handle_token']
        invocation.return_value(GLib.Variant('(o)', (request,)))
        def respond():
            conn.emit_signal(None, request, 'org.freedesktop.portal.Request', 'Response',
                             GLib.Variant('(ua{sv})', (0, {'uri': GLib.Variant('s', snapshot.as_uri())})))
            return False
        GLib.idle_add(respond)

    for interface in Gio.DBusNodeInfo.new_for_xml(xml).interfaces:
        connection.register_object(DESKTOP, interface, method, lambda *args: GLib.Variant('u', 2), None)

    def sample():
        nonlocal bundle, resident
        for proc in Path('/proc').glob('[0-9]*'):
            try:
                exe = Path(os.readlink(proc/'exe'))
                if exe.name not in ['addict', 'tesseract']: continue
                env = dict(x.split(b'=', 1) for x in (proc/'environ').read_bytes().split(b'\0') if b'=' in x)
                if env.get(b'XDG_DATA_HOME') != os.fsencode(out/'data'): continue
                if exe.name == 'addict':
                    resident = int(proc.name); bundle = Path(os.fsdecode(env[b'APPDIR']))
                elif bundle and exe.is_relative_to(bundle):
                    # A short-lived child can start exiting between reads: its
                    # memory goes first, so cmdline and maps then read empty
                    # while the PID remains. Read cmdline first and keep only
                    # samples where both are intact, so an exiting child never
                    # replaces a complete sample.
                    cmdline = [arg for arg in (proc/'cmdline').read_bytes().decode().split('\0') if arg]
                    maps = (proc/'maps').read_text()
                    if cmdline and 'libtesseract.so' in maps and 'libleptonica.so' in maps:
                        observed[proc.name] = {'exe': str(exe), 'cmdline': cmdline, 'maps': maps}
            except (OSError, KeyError): pass

    def wait(predicate, seconds=45):
        deadline = time.monotonic()+seconds
        while True:
            for _ in range(20):
                if not GLib.MainContext.default().pending(): break
                GLib.MainContext.default().iteration(False)
            sample()
            if predicate(): return
            if time.monotonic()>deadline: raise RuntimeError('verification timeout: '+str(out))
            time.sleep(.005)

    with (out/'launch.log').open('w') as log:
        launch = subprocess.Popen(['strace', '-f', '-yy', '-s', '2048', '-e', 'trace=execve,openat',
                                   '-o', str(out/'syscalls.log'), str(image), '--evidence', str(out/'application.jsonl'), '--origin', 'local'],
                                  stdout=log, stderr=log, start_new_session=True)
        try:
            wait(lambda: resident is not None)
            # AT-SPI invokes the native button and observes the GTK entry's
            # returned text. Only the accessibility service can activate on
            # this private bus; the real desktop portal is never contacted.
            capture_button = field = None
            def widgets():
                nonlocal capture_button, field
                def walk(node):
                    nonlocal capture_button, field
                    node.clearCache()
                    if node.name == 'Capture' and node.getRoleName() == 'push button': capture_button = node
                    if node.getState().contains(pyatspi.STATE_EDITABLE): field = node
                    for child in node: walk(child)
                desktop = pyatspi.Registry.getDesktop(0)
                desktop.clearCache()
                for app in desktop: walk(app)
                return capture_button is not None and field is not None
            print('waiting for native Capture and editable field', flush=True)
            wait(widgets)
            print('pressing native Capture', flush=True)
            assert capture_button.queryAction().doAction(0), 'native Capture action refused'
            wait(lambda: bool(shots))
            print('received production Screenshot request', flush=True)
            wait(lambda: bool(observed) and not (out/'region.png').exists())
            text = ''
            def returned():
                nonlocal text
                field.clearCache()
                text = field.queryText().getText(0, -1).strip()
                (out/'observed-text.txt').write_text(text+'\n')
                return text == TEXTS[lang][1]
            wait(returned, 20)
            print('asserted GTK field: '+text, flush=True)
            trace = (out/'syscalls.log').read_text()
            expected = str(bundle/'share/tessdata'/(lang+'.traineddata'))
            # The app passes bin/../share/tessdata; normalize the successful open literally recorded by strace.
            model_lines = [line for line in trace.splitlines() if lang+'.traineddata"' in line and '= -1' not in line]
            assert model_lines and any(str(bundle) in line for line in model_lines), 'missing bundled model open'
            assert str(bundle/'bin/tesseract') in trace, 'missing sibling child exec'
            for item in observed.values():
                for library in ['libtesseract.so', 'libleptonica.so', 'libc.so.6']:
                    paths = [line.split()[-1] for line in item['maps'].splitlines() if library in line]
                    assert paths and all(Path(p).is_relative_to(bundle) for p in paths), paths
                assert '-l' in item['cmdline'] and lang in item['cmdline']
            (out/'result.json').write_text(json.dumps({'language': lang, 'text': text, 'model': expected,
                                                      'model_opens': model_lines, 'children': observed, 'portal': shots}, indent=2)+'\n')
        finally:
            if resident:
                try: os.kill(resident, signal.SIGTERM)
                except ProcessLookupError: pass
            try: launch.wait(timeout=100)
            except subprocess.TimeoutExpired:
                os.killpg(launch.pid, signal.SIGTERM)
                try: launch.wait(timeout=5)
                except subprocess.TimeoutExpired: os.killpg(launch.pid, signal.SIGKILL); launch.wait(timeout=5)
            if bundle and bundle.exists(): raise RuntimeError('extraction cleanup failed: '+str(bundle))
            (out/'cleanup.txt').write_text('resident terminated; extraction tree absent: '+str(bundle)+'\n')


if __name__ == '__main__':
    if len(sys.argv) == 5 and sys.argv[1] == '--inside':
        inside(Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4])
    elif len(sys.argv) == 3:
        outer(Path(sys.argv[1]).resolve(strict=True), Path(sys.argv[2]).resolve())
    else: raise SystemExit('usage: ocr.py IMAGE NEW_EVIDENCE_DIR')
