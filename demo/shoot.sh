#!/bin/bash
# Sets up an omavm VM for the README screenshots and clips, and starts a take.
# Run the steps one by one; the shots themselves are judgement calls, see
# demo/README.md. Needs: omavm, a release build, demo/render.py output.
#
#   demo/shoot.sh setup <audio-dir>   boot, 4K at scale 2, install app + widget + audio
#   demo/shoot.sh take                start the screen recorder, the app and both tracks
#   demo/shoot.sh stop                stop the meeting (the transcribing part starts)
#   demo/shoot.sh end                 stop the screen recorder
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
bus='export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus'

mode() {
  # hyprctl reload and `omarchy theme set` put the monitor back; run this after both.
  omavm hypr eval 'hl.monitor({ output = "Virtual-1", mode = "3840x2160@60", scale = 2 })'
}

case "${1:-}" in
setup)
  audio="$2"
  omavm boot
  omavm push "$repo/target/release/omarchy-meeting-recorder" /usr/local/bin/omarchy-meeting-recorder
  omavm ssh 'mkdir -p /tmp/stage && chmod 755 /usr/local/bin/omarchy-meeting-recorder'
  for f in "$repo/data/omarchy-meeting-recorder.xml" "$repo/data/omarchy-meeting-recorder.desktop" \
    "$audio/maya.wav" "$audio/tom.wav"; do
    omavm push "$f" /tmp/stage/
  done
  omavm push ~/.local/share/voxtype/models/ggml-large-v3-turbo.bin /tmp/stage/model.bin
  omavm ssh 'chmod -R a+r /tmp/stage'
  omavm user "$bus
    mkdir -p ~/.local/share/omarchy-meeting-recorder/models ~/.local/share/mime/packages ~/Documents/Meetings ~/.local/state/omarchy-meeting-recorder
    cp /tmp/stage/model.bin ~/.local/share/omarchy-meeting-recorder/models/ggml-large-v3-turbo.bin
    cp /tmp/stage/omarchy-meeting-recorder.xml ~/.local/share/mime/packages/ && update-mime-database ~/.local/share/mime
    sed 's|^Exec=omarchy-meeting-recorder|Exec=/usr/local/bin/omarchy-meeting-recorder|' /tmp/stage/omarchy-meeting-recorder.desktop > ~/.local/share/applications/omarchy-meeting-recorder.desktop
    xdg-mime default omarchy-meeting-recorder.desktop application/x-omarchy-meeting
    grep -q OmarchyMeetingRecorder ~/.config/hypr/windows.lua || printf '%s\n' \
      'o.window(\"^com\\\\.jankeesvw\\\\.OmarchyMeetingRecorder\$\", { float = true })' \
      'o.window(\"^com\\\\.jankeesvw\\\\.OmarchyMeetingRecorder\$\", { size = { 480, 700 } })' \
      'o.window(\"^com\\\\.jankeesvw\\\\.OmarchyMeetingRecorder\$\", { center = true })' >> ~/.config/hypr/windows.lua
    echo '{\"format\":\"stereo\",\"language\":\"en\",\"your_name\":\"Maya\"}' > ~/.local/state/omarchy-meeting-recorder/settings.json
    # A virtual microphone for Maya; Tom plays on the default output, whose monitor is the computer audio.
    pactl load-module module-null-sink sink_name=fakemic >/dev/null
    pactl load-module module-remap-source master=fakemic.monitor source_name=demo_mic >/dev/null
    pactl load-module module-null-sink sink_name=demo_speakers >/dev/null
    pactl set-default-sink demo_speakers
    pactl set-default-source demo_mic
    omarchy-notification-dismiss 'Update System' || true
    omarchy-notification-dismiss 'Learn Keybindings' || true"
  omavm hypr reload
  omavm plugin "$repo/plugin"
  mode
  ;;
take)
  omavm user 'setsid nohup gpu-screen-recorder -w screen -f 30 -k h264 -cursor no -fallback-cpu-encoding yes -o /tmp/take.mp4 >/tmp/gsr.log 2>&1 < /dev/null & echo $! > /tmp/gsr.pid'
  sleep 2
  omavm user "$bus
    setsid nohup omarchy-meeting-recorder >/tmp/app.log 2>&1 < /dev/null &
    sleep 0.5
    setsid nohup paplay --device=fakemic /tmp/stage/maya.wav >/dev/null 2>&1 < /dev/null &
    setsid nohup paplay --device=demo_speakers /tmp/stage/tom.wav >/dev/null 2>&1 < /dev/null &"
  # Rename the meeting through QEMU, which reaches the focused field.
  sleep 3
  for k in shift-l a u n c h spc s y n c ret; do omavm sendkey "$k"; done
  ;;
stop)
  omavm user 'omarchy-meeting-recorder stop'
  ;;
end)
  omavm user 'kill -INT "$(cat /tmp/gsr.pid)"'
  ;;
*)
  sed -n '2,12p' "$0"
  exit 2
  ;;
esac
