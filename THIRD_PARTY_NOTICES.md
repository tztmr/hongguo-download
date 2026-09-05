# Third-Party Notices

The release bundle includes the following third-party software. Their respective
licenses apply in addition to this application's own terms.

## FFmpeg and ffprobe

FFmpeg and ffprobe are provided by the FFmpeg project: https://ffmpeg.org/

The release process accepts only a pinned ARM64 macOS archive whose two binaries
match release-supplied SHA-256 values. The distributor must retain the exact
source/build provenance and the license output from `ffmpeg -L` for that pinned
build. FFmpeg may be licensed under LGPL 2.1-or-later or GPL 2-or-later depending
on its configuration; the distributor is responsible for satisfying the terms
of the actual staged build and providing corresponding source when required.

## Python application sidecar

The bundled API sidecar contains Python and the packages pinned by
`requirements.txt` and `requirements-build.txt`. Package license metadata from
the release environment must accompany any public distribution.

## Optional AI components

Demucs, Whisper, PyTorch, and model weights are not included in the base app.
They are downloaded only after explicit confirmation from a checksum-pinned
HTTPS manifest. Their upstream licenses and model terms apply separately.
