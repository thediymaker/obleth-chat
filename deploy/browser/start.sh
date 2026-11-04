#!/bin/bash
# Starts Chromium with DevTools on 127.0.0.1:9223 and forwards port 9222 to it, the
# port Chat connects to. The namespace's network policy admits only Chat. The profile
# (cookies and sign-ins) lives on the person's volume at /profile.
set -u
profile=/profile/chrome
mkdir -p "$profile" "$HOME"
# A pod that stopped without closing Chromium leaves its lock behind.
rm -f "$profile"/Singleton*
major=$(chromium --version | grep -oE '[0-9]+' | head -n 1)
agent="Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/${major}.0.0.0 Safari/537.36"
chromium --headless --no-sandbox --disable-dev-shm-usage --disable-gpu \
  --user-data-dir="$profile" --remote-debugging-address=127.0.0.1 --remote-debugging-port=9223 \
  --no-first-run --no-default-browser-check --disable-sync --disable-component-update \
  --password-store=basic --window-size=1280,800 --lang=en-US --user-agent="$agent" \
  about:blank &
socat TCP-LISTEN:9222,fork,reuseaddr TCP:127.0.0.1:9223 &
# Either one ending restarts the container.
wait -n
exit 1
