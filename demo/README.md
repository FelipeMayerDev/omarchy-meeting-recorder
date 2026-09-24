# Demo assets

Everything in `screenshots/` and the social images was shot in a throwaway Omarchy VM (`omavm`) with an invented meeting, never with real recordings.

- `script.txt` is the meeting: Maya on the microphone, Tom on the computer audio, about five minutes about a product launch.
- `render.py` voices it with piper into two aligned tracks. It uses `en_US-amy-medium` for Maya and `en_US-ryan-medium` for Tom from [rhasspy/piper-voices](https://huggingface.co/rhasspy/piper-voices). Nothing is played; it only writes WAV files.
- `shoot.sh` sets up the VM (4K at scale 2, the app, the model, the MIME type, the bar widget, a virtual microphone) and starts a take: the screen recorder, the app and both tracks at the same moment.
- `social.sh` composes the 1600x900 social images from two card crops.
- `transitions.py` cuts the state-transition video (16:9 and square) from a take and a playback take; the times in it match the take and need adjusting after a new one.
- `details.py` turns a crop of a 4K capture into a close-up for X: a rounded card with a shadow on a dark field, with a caption.

## Shooting again

```bash
demo/render.py <voices-dir> /tmp/demo-audio
cargo build --release
demo/shoot.sh setup /tmp/demo-audio
demo/shoot.sh take          # stills with `omavm shot` while it records; `omarchy-meeting-recorder compact` for the strip
# wait until the audio is done (about five minutes), then:
demo/shoot.sh stop          # the transcribing animation; stills along the way
demo/shoot.sh end           # after the done screen appears
omavm pull /tmp/take.mp4 take.mp4
```

Chapters are not made inside the VM, because it has no agent logged in. Pull the meeting folder, run the chapter prompt from `src/chapters.rs` through `omarchy-meeting-recorder ask` on the host, write the result into the `.meeting-recorder` file and `transcript.md`, and push it back. Renaming "Remote" to "Tom" happens the same way.

Things to know:

- `hyprctl reload` and `omarchy theme set` reset the monitor to its odd default mode. Set 3840x2160 at scale 2 again afterwards (`shoot.sh` has the call).
- Keys sent with `omavm sendkey` reach the focused window. Shift+Tab from the done screen lands in the chapters list; arrows and Space start playback from a chapter.
- For social images, set a flat `#111119` background first (`omarchy theme bg set`), crop the window exactly, and run `social.sh <done-card> <animation-card> <out-dir>`.
- Put the original wallpaper back and stop the VM when done.
