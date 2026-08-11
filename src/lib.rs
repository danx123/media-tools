use pyo3::prelude::*;
use numpy::{PyArray3, IntoPyArray, ndarray::Array3};
use std::path::Path;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

// ═══════════════════════════════════════════════
// BAGIAN 1: BACA INFORMASI VIDEO DARI HEADER
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
        let path = Path::new(file_path);
        let mut file = File::open(path)
            .map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("Buka file: {}", e)))?;

        let mut buf = [0u8; 16];
        file.read_exact(&mut buf)
            .map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("Baca header: {}", e)))?;

        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();

        let (duration, width, height, fps, codec): (f64, u32, u32, f64, String) = if ext == "mp4" || ext == "m4v" || ext == "mov" {
            baca_mp4_info(&mut file)
        } else if ext == "mkv" || ext == "webm" {
            baca_mkv_info(&mut file)
        } else {
            // Untuk format lain: jalankan ffprobe DI WAKTU JALAN
            baca_ffprobe_dinamis(file_path)
        }?;

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
            "VideoInfo(dur={:.1}s, {}x{} @{:.2}fps, {})",
            self.duration, self.width, self.height, self.fps, self.codec
        )
    }
}

// ═══════════════════════════════════════════════
// BAGIAN 2: AMBIL BINGKAI → PAKAI FFmpeg DI SISTEM PENGGUNA
// ═══════════════════════════════════════════════

#[pyclass]
struct VideoFrameReader {
    path: String,
    #[pyo3(get)] duration: f64,
    #[pyo3(get)] width: u32,
    #[pyo3(get)] height: u32,
}

#[pymethods]
impl VideoFrameReader {
    #[new]
    fn new(file_path: &str) -> PyResult<Self> {
        let info = VideoInfo::new(file_path)?;
        Ok(VideoFrameReader {
            path: file_path.to_string(),
            duration: info.duration,
            width: info.width,
            height: info.height,
        })
    }

    /// Lompat ke detik tertentu → kembalikan bingkai
    fn seek_and_read(&mut self, second: f64) -> PyResult<Py<PyArray3<u8>>> {
        use std::process::Command;

        let output = Command::new("ffmpeg")
            .args(&[
                "-ss", &format!("{}", second),
                "-i", &self.path,
                "-vframes", "1",
                "-f", "rawvideo",
                "-pix_fmt", "rgb24",
                "-v", "quiet",
                "-"
            ])
            .output()
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("FFmpeg gagal: {}", e)))?;

        if !output.status.success() {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                format!("FFmpeg error: {}", String::from_utf8_lossy(&output.stderr))
            ));
        }

        let data = output.stdout;
        let total_pixels = self.width as usize * self.height as usize;
        let expected_len = total_pixels * 3;

        if data.len() < expected_len {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                format!("Data tidak cukup: {} < {}", data.len(), expected_len)
            ));
        }

        let arr = Array3::from_shape_vec(
            (self.height as usize, self.width as usize, 3),
            data[0..expected_len].to_vec(),
        )
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("Shape gagal: {}", e)))?;

        Python::with_gil(|py| {
            let py_arr: Py<PyArray3<u8>> = arr.into_pyarray_bound(py).into();
            Ok(py_arr)
        })
    }
}

// ═══════════════════════════════════════════════
// FUNGSI BANTU: BACA HEADER MP4
// ═══════════════════════════════════════════════

fn baca_mp4_info(file: &mut File) -> PyResult<(f64, u32, u32, f64, String)> {
    file.seek(SeekFrom::Start(0))?;
    let mut buf = [0u8; 8];

    let mut timescale = 1000;
    let mut duration = 0u64;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut codec = "MP4".to_string();

    loop {
        if file.read(&mut buf).is_err() || buf == [0; 8] { break; }
        let size = u32::from_be_bytes(buf[0..4].try_into().unwrap()) as u64;
        let kind = &buf[4..8];

        if size == 0 || size > 100_000_000 { break; }

        match kind {
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"stsd" => {
                // Masuk ke kotak ini — baca isinya
            }
            b"mvhd" => {
                let mut v = [0u8; 1];
                file.read_exact(&mut v)?;
                let _version = v[0];
                file.seek(SeekFrom::Current(12))?; // lewati waktu pembuatan dll
                let mut ts_buf = [0u8; 4];
                file.read_exact(&mut ts_buf)?;
                timescale = u32::from_be_bytes(ts_buf);
                let mut dur_buf = [0u8; 4];
                file.read_exact(&mut dur_buf)?;
                duration = u32::from_be_bytes(dur_buf) as u64;
                continue;
            }
            b"avc1" | b"mp4v" | b"hev1" | b"hvc1" | b"vp09" => {
                file.seek(SeekFrom::Current(78))?;
                let mut w_buf = [0u8; 2];
                file.read_exact(&mut w_buf)?;
                width = u16::from_be_bytes(w_buf) as u32;
                let mut h_buf = [0u8; 2];
                file.read_exact(&mut h_buf)?;
                height = u16::from_be_bytes(h_buf) as u32;
                codec = String::from_utf8_lossy(kind).to_string();
                continue;
            }
            _ => {}
        }

        if size > 8 {
            file.seek(SeekFrom::Current((size - 8) as i64))?;
        }
    }

    let dur_sec = if timescale > 0 { duration as f64 / timescale as f64 } else { 0.0 };
    Ok((dur_sec, width, height, 0.0, codec))
}

// ═══════════════════════════════════════════════
// FUNGSI BANTU: BACA MKV/WEBM
// ═══════════════════════════════════════════════

fn baca_mkv_info(_file: &mut File) -> PyResult<(f64, u32, u32, f64, String)> {
    // Sementara fallback ke ffprobe untuk mkv
    Err(pyo3::exceptions::PyValueError::new_err("Gunakan ffprobe untuk MKU"))
}

// ═══════════════════════════════════════════════
// FUNGSI BANTU: JALANKAN FFPROBE DINAMIS
// ═══════════════════════════════════════════════

fn baca_ffprobe_dinamis(path: &str) -> PyResult<(f64, u32, u32, f64, String)> {
    use std::process::Command;

    let output = Command::new("ffprobe")
        .args(&[
            "-v", "quiet",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height,r_frame_rate,duration,codec_name",
            "-of", "default=noprint_wrappers=1",
            path
        ])
        .output()
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("FFprobe tidak ditemukan: {}", e)))?;

    if !output.status.success() {
        return Err(pyo3::exceptions::PyRuntimeError::new_err("FFprobe gagal"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut width = 0u32;
    let mut height = 0u32;
    let mut duration = 0.0f64;
    let mut fps = 0.0f64;
    let mut codec = "Tidak diketahui".to_string();

    for baris in stdout.lines() {
        if let Some(val) = baris.strip_prefix("width=") {
            width = val.parse().unwrap_or(0);
        } else if let Some(val) = baris.strip_prefix("height=") {
            height = val.parse().unwrap_or(0);
        } else if let Some(val) = baris.strip_prefix("duration=") {
            duration = val.parse().unwrap_or(0.0);
        } else if let Some(val) = baris.strip_prefix("r_frame_rate=") {
            let parts: Vec<&str> = val.split('/').collect();
            if parts.len() == 2 {
                let n: f64 = parts[0].parse().unwrap_or(0.0);
                let d: f64 = parts[1].parse().unwrap_or(1.0);
                fps = if d > 0.0 { n / d } else { 0.0 };
            }
        } else if let Some(val) = baris.strip_prefix("codec_name=") {
            codec = val.to_string();
        }
    }

    Ok((duration, width, height, fps, codec))
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
