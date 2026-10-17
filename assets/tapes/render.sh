#!/bin/sh
# Render one tape to its GIF:  DRIFT=$PWD/target/release/drift sh assets/tapes/render.sh <name>
# (from the repo root; <name> is the tape's file name without .tape).
#
# vhs 0.12.0 never writes its GIF: it renders with the context it
# already cancelled to stop recording, so ffmpeg never starts — and it
# still exits 0. This runs a copy of the tape that also keeps the raw
# frames (vhs moves them into place before its cleanup), then encodes
# them with the filter graph vhs itself builds (video.go buildFFopts).
# Once vhs is fixed, `vhs assets/tapes/<name>.tape` does the same.
#
# The recording shell sheds herdr's and Claude Code's variables: a
# drift recorded from inside a real herdr session would otherwise see
# that session's agents, and a demo claude would think it was a child
# of the one running the render.
set -e

NAME=$1
TAPE=assets/tapes/$NAME.tape
[ -f "$TAPE" ] || { echo "usage: render.sh <tape name>  (no $TAPE)" >&2; exit 1; }
[ -n "$DRIFT" ] || { echo "set DRIFT to the drift binary" >&2; exit 1; }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
FRAMES=$WORK/frames.png
GIF=$(sed -n 's/^Output \(.*\.gif\)$/\1/p' "$TAPE")
W=$(sed -n 's/^Set Width \([0-9]*\)$/\1/p' "$TAPE")
H=$(sed -n 's/^Set Height \([0-9]*\)$/\1/p' "$TAPE")
P=$(sed -n 's/^Set Padding \([0-9]*\)$/\1/p' "$TAPE")
P=${P:-60}
BG=$(sed -n 's/^Set Theme .*"background": "\(#[0-9a-fA-F]*\)".*/\1/p' "$TAPE")
BG=${BG:-#171717}

sed "s#^Output .*\.gif\$#&\nOutput \"$FRAMES\"#" "$TAPE" > "$WORK/$NAME.tape"
env $(env | awk -F= '/^(HERDR_|CLAUDE|AI_AGENT)/{printf "-u %s ", $1}') \
  vhs "$WORK/$NAME.tape" > "$WORK/vhs.log" 2>&1 || { cat "$WORK/vhs.log" >&2; exit 1; }
[ -f "$FRAMES/frame-text-00001.png" ] || { cat "$WORK/vhs.log" >&2; echo "no frames recorded" >&2; exit 1; }

ffmpeg -loglevel error -y \
  -r 50 -start_number 1 -i "$FRAMES/frame-text-%05d.png" \
  -r 50 -start_number 1 -i "$FRAMES/frame-cursor-%05d.png" \
  -filter_complex "
    [0][1]overlay[merged];
    [merged]scale=$((W - 2 * P)):$((H - 2 * P)):force_original_aspect_ratio=1[scaled];
    [scaled]fps=50,setpts=PTS/1.000000[speed];
    [speed]pad=$W:$H:(ow-iw)/2:(oh-ih)/2:$BG[padded];
    [padded]fillborders=left=$P:right=$P:top=$P:bottom=$P:mode=fixed:color=$BG[padded];
    [padded]split[plt_a][plt_b];
    [plt_a]palettegen=max_colors=256[plt];
    [plt_b][plt]paletteuse[palette]" \
  -map "[palette]" "$GIF"
echo "$GIF: $(ls "$FRAMES" | grep -c text) frames, $(du -h "$GIF" | cut -f1)"
