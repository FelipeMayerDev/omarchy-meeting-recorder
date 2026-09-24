#!/usr/bin/env python3
"""Cuts the state-transition video from two VM takes (3840x2160, scale 2).

    demo/transitions.py <take.mp4> <playback-take.mp4> <out-dir>

The times below match the take described in demo/README.md: launch at 0:03,
compact 0:32 to 0:39, stop at 0:48, lines in the animation from 1:36, the
demo meeting opened from 2:09. Adjust them after a new take. Writes
transitions.mp4 (1920x1080) and transitions-square.mp4 (1080x1080).
"""

import subprocess
import sys
import tempfile
from pathlib import Path

# Crop rectangles (w, h, x, y) in the 4K capture, per window state.
WIDE = {
    "full": (2774, 1560, 533, 326),
    "compact": (1600, 900, 1120, 656),
    "done": (2916, 1640, 462, 288),
}
SQUARE = {
    "full": (1560, 1560, 1140, 326),
    "compact": (900, 900, 1470, 656),
    "done": (2160, 2160, 840, 0),
}
FADE = 0.3


def segments(take: str, playback: str):
    # (source, start, end, framing, speed)
    return [
        (take, 2.6, 6.6, "full", 1),        # recording starts, meters moving
        (take, 32.3, 35.5, "compact", 1),   # the compact strip
        (take, 40.3, 42.5, "full", 1),      # back to full
        (take, 46.8, 50.3, "full", 1),      # stop, saving audio
        (take, 50.3, 94.3, "full", 11),     # transcribing, sped up
        (take, 95.5, 99.8, "full", 1),      # the lines typing in
        (take, 129.5, 132.5, "done", 1),    # the done screen with chapters
        (playback, 3.0, 9.5, "done", 1),    # playing from the first chapter
    ]


def render(take, playback, out: Path, frames: dict, size: tuple, name: str):
    w, h = size
    with tempfile.TemporaryDirectory() as tmp:
        parts = []
        for i, (src, start, end, framing, speed) in enumerate(segments(take, playback)):
            cw, ch, cx, cy = frames[framing]
            part = Path(tmp) / f"{i}.mp4"
            vf = f"crop={cw}:{ch}:{cx}:{cy},scale={w}:{h}:flags=lanczos,setpts=PTS/{speed},fps=30,format=yuv420p"
            subprocess.run(["ffmpeg", "-v", "error", "-y", "-ss", str(start), "-to", str(end), "-i", src,
                            "-vf", vf, "-an", "-c:v", "libx264", "-crf", "14", "-preset", "fast", str(part)],
                           check=True)
            dur = float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration",
                                        "-of", "csv=p=0", str(part)], capture_output=True, text=True).stdout)
            parts.append((part, dur, framing))
        # Crossfade where the framing changes, hard cut where it stays.
        inputs, chain, offset, last = [], "", 0.0, "[0:v]"
        for i, (part, dur, framing) in enumerate(parts):
            inputs += ["-i", str(part)]
            if i == 0:
                offset = dur
                continue
            fade = FADE if framing != parts[i - 1][2] else 0.04
            offset -= fade
            label = f"[v{i}]"
            chain += f"{last}[{i}:v]xfade=transition=fade:duration={fade}:offset={offset:.3f}{label};"
            last, offset = label, offset + dur
        subprocess.run(["ffmpeg", "-v", "error", "-y", *inputs, "-filter_complex", chain.rstrip(";"),
                        "-map", last, "-c:v", "libx264", "-preset", "slow", "-crf", "22",
                        "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(out / name)], check=True)


def main():
    take, playback, out = sys.argv[1], sys.argv[2], Path(sys.argv[3])
    render(take, playback, out, WIDE, (1920, 1080), "transitions.mp4")
    render(take, playback, out, SQUARE, (1080, 1080), "transitions-square.mp4")


if __name__ == "__main__":
    main()
