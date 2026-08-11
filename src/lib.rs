use pyo3::prelude::*;
use numpy::PyArray3;
use ffmpeg_next as ffmpeg;
use std::path::Path;

// ═══════════════════════════════════════════════
// BAGIAN 1: BACA INFORMASI VIDEO (GANTI FFPROBE)
// ═══════════════════════════════════════════════

#[pyclass]
struct VideoInfo {
    #[pyo3(get)] path: String,
    #[pyo3(get)] duration: f64,
    #[pyo3(get)] width: u32,
    #[pyo3(get)] height: u32,
    #[pyo3(get)] fps: f64,
    #[pyo3(get)] codec: String,
}

#[pymethods]
impl VideoInfo {
    #[new]
    fn new(file_path: &str) -> PyResult<Self> {
        ffmpeg::init().map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("FFmpeg init: {}", e)))?;

        let path = Path::new(file_path);
        let mut input = ffmpeg::format::input(&path)
            .map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("Buka file: {}", e)))?;

        let stream = input.streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("Tidak ada aliran video"))?;

        let codec = ffmpeg::codec::context::Parameters::from(stream)
            .id()
            .description()
            .unwrap_or("Tidak diketahui")
            .to_string();

        let fps = stream.avg_frame_rate();
        let fps = if fps.denominator() > 0 {
            fps.numerator() as f64 / fps.denominator() as f64
        } else { 0.0 };

        let duration = input.duration() as f64 / 1_000_000.0;

        let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("Konteks dekoder: {}", e)))?;

        let (width, height) = if let Some(video) = decoder.video() {
            (video.width(), video.height())
        } else { (0, 0) };

        Ok(VideoInfo {
            path: file_path.to_string(),
            duration,
            width,
            height,
            fps,
            codec,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "VideoInfo(dur={:.1f}s, {}x{} @{:.2f}fps, {})",
            self.duration, self.width, self.height, self.fps, self.codec
        )
    }
}

// ═══════════════════════════════════════════════
// BAGIAN 2: BACA BINGKAI / THUMBNAIL (GANTI OPENCV)
// ═══════════════════════════════════════════════

#[pyclass]
struct VideoFrameReader {
    input_context: ffmpeg::format::context::Input,
    stream_index: usize,
    decoder: ffmpeg::codec::context::decoder::Video,
    time_base: f64,
    #[pyo3(get)] duration: f64,
    #[pyo3(get)] width: u32,
    #[pyo3(get)] height: u32,
}

#[pymethods]
impl VideoFrameReader {
    #[new]
    fn new(file_path: &str) -> PyResult<Self> {
        ffmpeg::init().map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("FFmpeg init: {}", e)))?;

        let path = Path::new(file_path);
        let mut input_context = ffmpeg::format::input(&path)
            .map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("Buka file: {}", e)))?;

        let (stream_index, stream) = input_context.streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("Tidak ada aliran video"))?
            .into();

        let tb = stream.time_base();
        let time_base = if tb.denominator() > 0 {
            tb.numerator() as f64 / tb.denominator() as f64
        } else { 0.0 };

        let duration = stream.duration() as f64 * time_base;

        let codec_ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("Konteks dekoder: {}", e)))?;

        let decoder = codec_ctx.decoder()
            .video()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("Buat dekoder: {}", e)))?;

        let (width, height) = (decoder.width(), decoder.height());

        Ok(VideoFrameReader {
            input_context,
            stream_index,
            decoder,
            time_base,
            duration,
            width,
            height,
        })
    }

    /// Lompat ke detik tertentu → kembalikan bingkai sebagai np.array [H, W, 3]
    fn seek_and_read(&mut self, second: f64) -> PyResult<Py<PyArray3<u8>>> {
        let target_ts = (second / self.time_base).round() as i64;

        self.input_context.seek(target_ts..target_ts + 5)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("Gagal lompat: {}", e)))?;

        let mut pkt = ffmpeg::packet::Packet::empty();
        loop {
            match self.input_context.read(&mut pkt) {
                Ok(_) => {
                    if pkt.stream_index() != self.stream_index { continue; }
                    if pkt.decode(&mut self.decoder).is_ok() {
                        if let Ok(frame) = self.decoder.frame() {
                            let data = frame.data(0);
                            let (w, h) = (frame.width() as usize, frame.height() as usize);
                            Python::with_gil(|py| {
                                Ok(PyArray3::from_vec(py, data.to_vec(), [h, w, 3]).into())
                            })
                        } else { continue; }
                    }
                }
                Err(ffmpeg::Error::Eof) => break,
                Err(e) => return Err(pyo3::exceptions::PyRuntimeError::new_err(format!("Baca gagal: {}", e))),
            }
        }

        Err(pyo3::exceptions::PyRuntimeError::new_err("Tidak dapat membaca bingkai"))
    }
}

// ═══════════════════════════════════════════════
// DAFTARKAN KE MODUL
// ═══════════════════════════════════════════════

#[pymodule]
fn media_tools(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<VideoInfo>()?;
    m.add_class::<VideoFrameReader>()?;
    Ok(())
}