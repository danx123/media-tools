# media_tools

A native Rust extension module for Python, built with [PyO3](https://pyo3.rs) and [maturin](https://www.maturin.rs), that provides fast video metadata inspection and frame extraction.

## Features

- **`VideoInfo`** — Reads video metadata (duration, width, height, fps, codec) directly from container headers.
  - Native binary parsing for `.mp4`, `.m4v`, and `.mov` (reads the `moov`/`mvhd`/`stsd` atoms directly, no external process required).
  - Falls back to `ffprobe` for `.mkv`, `.webm`, and any other format not natively supported.
- **`VideoFrameReader`** — Seeks to an arbitrary timestamp in a video and returns the decoded frame as a NumPy array (`H x W x 3`, `RGB24`), using `ffmpeg` under the hood.
- Compiled as a release build with LTO, single codegen unit, and stripped symbols for a small, fast binary.

## Requirements

- Python 3.13 (or adjust the target version in CI as needed)
- Rust (stable toolchain)
- [`ffmpeg`](https://ffmpeg.org) and [`ffprobe`](https://ffmpeg.org) available on the system `PATH` at runtime
  - Required for `VideoFrameReader.seek_and_read`
  - Required as a fallback for `VideoInfo` on formats without native parsing (e.g. MKV/WebM)

## Installation

### From a pre-built wheel

Download the wheel produced by the CI workflow (see [Building](#building)) and install it with pip:

```bash
pip install media_tools-*.whl
```

### From source

```bash
pip install maturin
maturin develop --release
```

## Usage

### Reading video metadata

```python
from media_tools import VideoInfo

info = VideoInfo("path/to/video.mp4")

print(info.path)      # "path/to/video.mp4"
print(info.duration)  # duration in seconds (float)
print(info.width)     # pixel width
print(info.height)    # pixel height
print(info.fps)       # frames per second
print(info.codec)     # e.g. "avc1", "hev1"
print(info)            # VideoInfo(dur=12.3s, 1920x1080 @29.97fps, avc1)
```

### Extracting a frame at a given timestamp

```python
from media_tools import VideoFrameReader

reader = VideoFrameReader("path/to/video.mp4")

# frame is a NumPy array of shape (height, width, 3), dtype=uint8, RGB order
frame = reader.seek_and_read(5.0)  # frame at 5.0 seconds

print(frame.shape)  # (1080, 1920, 3)
```

## API Reference

### `VideoInfo(file_path: str)`

| Property   | Type    | Description                                  |
|------------|---------|-----------------------------------------------|
| `path`     | `str`   | The path the object was created with          |
| `duration` | `float` | Duration in seconds                           |
| `width`    | `int`   | Frame width in pixels                         |
| `height`   | `int`   | Frame height in pixels                        |
| `fps`      | `float` | Frames per second                             |
| `codec`    | `str`   | Codec identifier (e.g. `avc1`, `hev1`, `vp09`) |

Raises `IOError` if the file cannot be opened, and `RuntimeError` if metadata cannot be determined (e.g. `ffprobe` is missing or fails).

### `VideoFrameReader(file_path: str)`

| Property   | Type    | Description                         |
|------------|---------|--------------------------------------|
| `duration` | `float` | Duration in seconds                  |
| `width`    | `int`   | Frame width in pixels                |
| `height`   | `int`   | Frame height in pixels               |

#### `seek_and_read(second: float) -> numpy.ndarray`

Seeks to the given timestamp and decodes a single frame via `ffmpeg`, returning it as an `(H, W, 3)` `uint8` NumPy array in RGB order. Raises `RuntimeError` if `ffmpeg` is unavailable, fails, or returns insufficient data.

## Building

This project is built with [maturin](https://www.maturin.rs) and packaged as a Python wheel. The included GitHub Actions workflow (`build.yml`) builds a Windows wheel on every push to `main`/`master`:

```bash
pip install maturin
maturin build --release --out dist
```

The resulting `.whl` file will be available in the `dist/` directory, and is uploaded as a build artifact named `media-tools-windows` in CI.

### Release profile

The release profile is tuned for performance and binary size:

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
strip = true
panic = "abort"
```

## Project Structure

```
.
├── Cargo.toml         # Rust crate manifest (PyO3, numpy, mp4, matroska)
├── src/
│   └── lib.rs          # VideoInfo and VideoFrameReader implementations
└── .github/
    └── workflows/
        └── build.yml   # CI: build wheel with maturin on Windows
```

## Notes & Limitations

- Native MP4/MOV header parsing extracts width/height and codec from the `stsd` box, but currently does not compute `fps` from the `mp4`/`mov` header — `fps` is only populated when falling back to `ffprobe`.
- Native MKV/WebM parsing is not yet implemented; these formats always fall back to `ffprobe`.
- `VideoFrameReader.seek_and_read` shells out to `ffmpeg` per call, so it is best suited for occasional/random-access frame extraction rather than sequential frame-by-frame decoding of an entire video.


