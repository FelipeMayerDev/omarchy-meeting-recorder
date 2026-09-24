#!/bin/bash
# Composes the 1600x900 social images from card crops shot on a flat #111119
# background (see demo/README.md). Usage:
#   demo/social.sh <done-card.png> <animation-card.png> <out-dir>
set -euo pipefail

done_card="$1"
anim_card="$2"
out="$3"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

magick -size 1600x900 radial-gradient:'#1B1B27'-'#111119' "$work/bg.png"
magick -background none /usr/share/omarchy/logo.svg -resize x34 \
  -fill '#6E6E80' -colorize 100 "$work/logo.png"

# card <in> <height> <out>: scaled card with a soft shadow under it.
card() {
  magick "$1" -resize "x$2" "$work/c.png"
  magick "$work/c.png" \( +clone -background black -shadow 65x28+0+14 \) \
    +swap -background none -layers merge +repage "$3"
}

# compose <card-with-shadow> <background> <title> <subtitle> <out> [text colour] [sub colour]
compose() {
  magick "$2" \
    "$1" -gravity north -geometry +0+56 -composite \
    "$work/logo.png" -gravity southeast -geometry +48+40 -composite \
    -font Liberation-Sans-Bold -pointsize 46 -fill "${6:-white}" -gravity north -annotate +0+712 "$3" \
    -font Liberation-Sans -pointsize 25 -fill "${7:-#8B8B9E}" -gravity north -annotate +0+778 "$4" \
    -strip -depth 8 "$5"
}

card "$done_card" 560 "$work/done.png"
card "$anim_card" 560 "$work/anim.png"
# A bigger done card for the split layout, bleeding off the right edge.
card "$done_card" 800 "$work/big.png"

compose "$work/done.png" "$work/bg.png" "Record meetings on Omarchy" \
  "Mic and computer audio, transcribed after the call, with a player built in" \
  "$out/omarchy-meeting-recorder-social-1.png"
compose "$work/anim.png" "$work/bg.png" "Transcribed on your own machine" \
  "Whisper runs locally when the call ends, and it looks good doing it" \
  "$out/omarchy-meeting-recorder-social-2.png"
# Split: the claim on the left, the chapters column of the card on the right.
magick "$work/bg.png" \
  "$work/big.png" -gravity northwest -geometry +720+50 -composite \
  "$work/logo.png" -gravity southwest -geometry +80+48 -composite \
  -font Liberation-Sans-Bold -pointsize 52 -fill white -gravity northwest \
  -annotate +80+300 "Chapters by" -annotate +80+364 "your default agent" \
  \( -background none -font Liberation-Sans -pointsize 26 -fill '#8B8B9E' \
     -size 560x caption:"Claude Code, Codex or whichever agent Omarchy is set up with divides the meeting into chapters, run without any tools." \) \
  -gravity northwest -geometry +80+450 -composite \
  -strip -depth 8 "$out/omarchy-meeting-recorder-social-3.png"

# A bright field with a faint, tilted lattice of microphones.
curl -sSL -o "$work/icon.svg" \
  "https://raw.githubusercontent.com/tailwindlabs/heroicons/master/optimized/24/outline/microphone.svg"
sed -i 's/currentColor/#ffffff/g' "$work/icon.svg"
magick -background none -density 600 "$work/icon.svg" -resize 64x64 \
  -alpha set -channel A -evaluate multiply 0.16 +channel "$work/icon.png"
python3 - "$work/icon.png" "$work/bright.png" <<'EOF'
import math, random, sys
from PIL import Image
icon = Image.open(sys.argv[1]).convert("RGBA"); W, H, CELL, TILT = 1600, 900, 150, math.radians(17)
field = Image.new("RGBA", (W, H), "#FF2D78")
random.seed(3)
reach = int(math.hypot(W, H) / CELL / 2) + 2
ct, st = math.cos(TILT), math.sin(TILT)
for j in range(-reach, reach + 1):
    for i in range(-reach, reach + 1):
        lx, ly = i * CELL, j * CELL
        cx = W / 2 + lx * ct - ly * st
        cy = H / 2 + lx * st + ly * ct
        if -CELL < cx < W + CELL and -CELL < cy < H + CELL:
            mark = icon.rotate(random.uniform(-6, 6), resample=Image.BICUBIC, expand=True)
            field.alpha_composite(mark, (int(cx - mark.width / 2), int(cy - mark.height / 2)))
field.convert("RGB").save(sys.argv[2])
EOF
magick -background none /usr/share/omarchy/logo.svg -resize x34 -fill white -colorize 100 "$work/logo.png"
compose "$work/done.png" "$work/bright.png" "Record meetings on Omarchy" \
  "Two tracks, a local transcript, chapters and a player" \
  "$out/omarchy-meeting-recorder-social-4.png" white '#FFE3EE'
ls -la "$out"/omarchy-meeting-recorder-social-*.png
