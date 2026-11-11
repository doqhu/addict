#!/usr/bin/env python3
"""Interactive, isolated desktop relaunch acceptance; never uses the real portal.

Only the mock below writes its disposable autostart entry. The application must
request every startup change itself through its Settings switch.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

APP_ID = 'io.github.doqhu.Addict'
DESKTOP = '/org/freedesktop/portal/desktop'


def outer(image, out):
    if not (os.environ.get('DISPLAY') or os.environ.get('WAYLAND_DISPLAY')):
        raise SystemExit('a desktop display is required')
    out.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    for key in ['APPIMAGE', 'APPDIR', 'ARG0', 'APPIMAGE_EXTRACT_AND_RUN',
                'APPIMAGE_TARGET_DIR', 'TARGET_APPIMAGE', 'DBUS_SESSION_BUS_ADDRESS',
                'GTK_PATH', 'GTK_MODULES', 'LD_LIBRARY_PATH']:
        env.pop(key, None)
    for key, name in [('HOME', 'home'), ('XDG_DATA_HOME', 'data'),
                      ('XDG_CONFIG_HOME', 'config'), ('XDG_CACHE_HOME', 'cache'),
                      ('XDG_RUNTIME_DIR', 'runtime')]:
        path = out / name
        path.mkdir(mode=0o700)
        env[key] = str(path)
    wayland = os.environ.get('WAYLAND_DISPLAY')
    if wayland and not Path(wayland).is_absolute():
        (out / 'runtime' / wayland).symlink_to(Path(os.environ['XDG_RUNTIME_DIR']) / wayland)
    env['XAUTHORITY'] = os.environ.get('XAUTHORITY', str(Path.home() / '.Xauthority'))
    images = out / 'images with spaces'
    images.mkdir()
    target = images / image.name
    shutil.copy2(image, target)
    (out / 'image.sha256').write_text(hashlib.file_digest(target.open('rb'), 'sha256').hexdigest() + '\n')
    (out / 'host.txt').write_text(Path('/etc/os-release').read_text() + '\n' + os.uname().release +
                                '\n/dev/fuse: ' + str(Path('/dev/fuse').exists()) + '\n')
    config = out / 'bus.conf'
    config.write_text('<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>'
                     '<auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/>'
                     '<allow receive_sender="*"/><allow own="*"/></policy></busconfig>')
    return subprocess.call(['dbus-run-session', '--config-file', str(config), '--',
                            sys.executable, __file__, '--inside', str(target), str(out)], env=env)


def inside(image, out):
    import gi
    gi.require_version('Gio', '2.0')
    from gi.repository import Gio, GLib
    (out / 'bus-address').write_text(os.environ['DBUS_SESSION_BUS_ADDRESS'])
    xml = '''<node>
    <interface name="org.freedesktop.host.portal.Registry"><method name="Register">
    <arg type="s" direction="in"/><arg type="a{sv}" direction="in"/></method></interface>
    <interface name="org.freedesktop.portal.Background">
    <property name="version" type="u" access="read"/>
    <method name="RequestBackground"><arg type="s" direction="in"/>
    <arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method></interface>
    </node>'''
    connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
                         'org.freedesktop.DBus', 'RequestName',
                         GLib.Variant('(su)', ('org.freedesktop.portal.Desktop', 0)),
                         None, Gio.DBusCallFlags.NONE, 5000, None)
    info = Gio.DBusNodeInfo.new_for_xml(xml)
    registered, requests, residents = set(), [], []
    launcher = out / 'data/applications' / (APP_ID + '.desktop')
    login = out / 'config/autostart' / (APP_ID + '.desktop')
    command = None

    def stage(name, instruction=''):
        (out / 'stage.json').write_text(json.dumps({'stage': name, 'instruction': instruction}, indent=2))
        print(name + ': ' + instruction, flush=True)

    def method(conn, sender, path, interface, name, parameters, invocation):
        nonlocal command
        if name == 'Register':
            assert parameters.unpack()[0] == APP_ID
            assert launcher.is_file(), 'launcher must exist at registration'
            registered.add(sender)
            invocation.return_value(GLib.Variant('()', ()))
            return
        assert sender in registered
        options = parameters.unpack()[1]
        enabled = options['autostart']
        items = options.get('commandline', [])
        assert launcher.is_file(), 'launcher must remain until portal disables'
        if enabled:
            assert len(items) == 2 and items[1] == '--background', items
            assert str(image) in items[0], items
            assert 'appimage_extracted_' not in items[0], items
            command = items
            key = GLib.KeyFile()
            key.set_string('Desktop Entry', 'Type', 'Application')
            key.set_string('Desktop Entry', 'Name', 'Addict')
            key.set_string('Desktop Entry', 'Exec', ' '.join(items))
            key.set_string('Desktop Entry', 'X-XDP-Autostart', APP_ID)
            login.parent.mkdir(parents=True, exist_ok=True)
            login.write_text(key.to_data()[0])
        else:
            assert items == [], items
            login.unlink(missing_ok=True)
        requests.append({'enabled': enabled, 'command': items, 'sender': sender})
        (out / 'portal.json').write_text(json.dumps(requests, indent=2))
        request = DESKTOP + '/request/' + sender.lstrip(':').replace('.', '_') + '/' + options['handle_token']
        invocation.return_value(GLib.Variant('(o)', (request,)))
        def respond():
            conn.emit_signal(None, request, 'org.freedesktop.portal.Request', 'Response',
                             GLib.Variant('(ua{sv})', (0, {'background': GLib.Variant('b', True),
                                                          'autostart': GLib.Variant('b', enabled)})))
            return False
        GLib.idle_add(respond)

    for interface in info.interfaces:
        connection.register_object(DESKTOP, interface, method,
                                   lambda *args: GLib.Variant('u', 2), None)

    def wait(predicate, timeout=300):
        deadline = time.monotonic() + timeout
        while not predicate():
            while GLib.MainContext.default().pending():
                GLib.MainContext.default().iteration(False)
            if time.monotonic() > deadline:
                raise RuntimeError('timeout; see stage.json and launch logs')
            time.sleep(0.05)

    def run(args, logname):
        with (out / logname).open('w') as log:
            child = subprocess.Popen(args, stdout=log, stderr=log)
            wait(lambda: child.poll() is not None, 120)
            assert child.returncode == 0, logname

    def find_resident():
        for proc in Path('/proc').glob('[0-9]*'):
            try:
                if Path(os.readlink(proc / 'exe')).name == 'addict' and (
                    'XDG_DATA_HOME=' + str(out / 'data')).encode() in (proc / 'environ').read_bytes().split(b'\0'):
                    return proc
            except OSError:
                pass
        return None

    def launched(label):
        wait(lambda: find_resident() is not None, 45)
        proc = find_resident()
        residents.append(int(proc.name))
        env = dict(item.split(b'=', 1) for item in (proc / 'environ').read_bytes().split(b'\0') if b'=' in item)
        bundle = Path(os.fsdecode(env[b'APPDIR']))
        assert os.fsdecode(env[b'APPIMAGE']) == str(image)
        assert Path(os.readlink(proc / 'exe')).is_relative_to(bundle)
        trace = out / label
        trace.mkdir()
        for name in ['maps', 'cmdline', 'environ']:
            (trace / name).write_bytes((proc / name).read_bytes())
        (trace / 'exe').write_text(os.readlink(proc / 'exe'))
        (trace / 'appdir').write_text(str(bundle))
        return int(proc.name), bundle

    def stop(pid, bundle):
        os.kill(pid, signal.SIGTERM)
        wait(lambda: not Path('/proc', str(pid)).exists() and not bundle.exists(), 100)
        assert not bundle.exists()
        (out / 'cleanup.log').open('a').write(f'{pid}: {bundle} removed\n')

    try:
        run([str(image), 'install'], 'install.log')
        key = GLib.KeyFile()
        key.load_from_file(str(launcher), GLib.KeyFileFlags.NONE)
        assert key.get_string('Desktop Entry', 'Exec') == '"' + str(image) + '"'
        (out / 'launcher.desktop').write_text(launcher.read_text())
        assert not login.exists()
        assert Gio.DesktopAppInfo.new_from_filename(str(launcher)).launch([], None)
        pid, bundle = launched('initial')
        stage('enable-startup', 'Observe the normal window, close the download dialog if needed, open Settings and enable login startup.')
        wait(lambda: len(requests) == 1)
        saved = out / 'config/addict/login-startup'
        wait(lambda: saved.is_file() and saved.read_text().strip() == 'enabled')
        (out / 'saved-command.json').write_text(json.dumps(command, indent=2))
        stop(pid, bundle)
        assert Gio.DesktopAppInfo.new_from_filename(str(launcher)).launch([], None)
        pid, bundle = launched('launcher-relaunch')
        stage('confirm-launcher', 'Observe the normal window, then create ' + str(out / 'launcher-confirmed'))
        wait(lambda: (out / 'launcher-confirmed').exists())
        stop(pid, bundle)
        assert Gio.DesktopAppInfo.new_from_filename(str(login)).launch([], None)
        pid, bundle = launched('background-relaunch')
        stage('confirm-background', 'Confirm that no app window opened, then create ' + str(out / 'background-confirmed'))
        wait(lambda: (out / 'background-confirmed').exists())
        stage('disable-enable', 'Open Settings, disable login startup and enable it again. This also verifies off/on repair.')
        wait(lambda: len(requests) == 3)
        assert [r['enabled'] for r in requests] == [True, False, True]
        wait(lambda: saved.is_file() and saved.read_text().strip() == 'enabled')
        stop(pid, bundle)
        run([str(image), 'uninstall'], 'uninstall.log')
        assert [r['enabled'] for r in requests] == [True, False, True, False]
        assert not launcher.exists() and not login.exists()
        assert not (out / 'config/addict/login-startup').exists()
        stage('PASS', 'Launcher and Background portal commands relaunched after extraction cleanup; Settings disable and uninstall confirmed.')
    finally:
        proc = find_resident()
        if proc:
            env = dict(item.split(b'=', 1) for item in (proc / 'environ').read_bytes().split(b'\0') if b'=' in item)
            stop(int(proc.name), Path(os.fsdecode(env[b'APPDIR'])))


if __name__ == '__main__':
    if len(sys.argv) == 4 and sys.argv[1] == '--inside':
        inside(Path(sys.argv[2]), Path(sys.argv[3]))
    elif len(sys.argv) == 3:
        sys.exit(outer(Path(sys.argv[1]).resolve(strict=True), Path(sys.argv[2]).resolve()))
    else:
        raise SystemExit('usage: relaunch.py IMAGE NEW_EVIDENCE_DIR')
