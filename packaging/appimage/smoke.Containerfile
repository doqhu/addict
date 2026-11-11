FROM docker.io/library/ubuntu@sha256:78d4744ead1f5b19b04d0df558ab182a5300903900755e515b2ca4d10eb2044d
# Only verification tools: no system Tesseract, models, desktop or FUSE.
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    python3 python3-gi python3-pyatspi at-spi2-core strace dbus-x11 xvfb xauth \
    && rm -rf /var/lib/apt/lists/*
ENTRYPOINT ["bash", "/checkout/scripts/check-appimage-smoke.sh", "--inside"]
