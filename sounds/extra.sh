#!/usr/bin/env bash
# Adds tones for the prompts the default theme (generate.sh) leaves spoken but that come up often:
# the noise canceling / ambient sound button, and "device replaced" when a third device takes over a
# multipoint slot. Run it after `fix-whxm4-voice generate`.
#
# Usage: extra.sh [changes-dir]   (default: ./changes)
#
#   06 noise canceling     one low dot
#   08 ambient sound       one high dot
#   07 ambient sound off   two middle dots
#   51/52 device replaced  the theme's "device connected" tone for that slot (47/49)
set -euo pipefail

out=${1:-changes}

# A short sine dot at $1 Hz with click-free fades, a little quieter than the theme's tones.
dot() {
    echo "sine=f=$1:d=0.07:sample_rate=48000,afade=t=in:d=0.004,afade=t=out:st=0.02:d=0.05,volume=-14dB"
}

ffmpeg -v error -y -f lavfi -i "$(dot 440)" -af apad=pad_dur=0.1 "$out/06-noise-canceling.wav"
ffmpeg -v error -y -f lavfi -i "$(dot 880)" -af apad=pad_dur=0.1 "$out/08-ambient-sound.wav"
ffmpeg -v error -y -filter_complex \
    "$(dot 660)[a];$(dot 660)[b];[a]apad=pad_dur=0.06[a2];[a2][b]concat=n=2:v=0:a=1,apad=pad_dur=0.1" \
    "$out/07-ambient-sound-off.wav"

for pair in 47:51-device-1-replaced 49:52-device-2-replaced; do
    from=$(find "$out" -maxdepth 1 -name "${pair%%:*}-*" | head -n 1)
    if [[ -z $from ]]; then
        echo "no ${pair%%:*}-* clip in $out; run \`fix-whxm4-voice generate\` first" >&2
        exit 1
    fi
    cp "$from" "$out/${pair#*:}.${from##*.}"
done

echo "added 06, 07, 08, 51 and 52 to $out"
