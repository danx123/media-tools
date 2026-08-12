use pyo3::prelude::*;
use numpy::{PyArray3, IntoPyArray, ndarray::Array3};
use std::path::Path;
use std::fs::File;
use std::io::BufReader;

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
    fn new(py: Python<'_>, file_path: &str) -> PyResult<Self> {
        // PENTING: seluruh pekerjaan di bawah (baca file, dan untuk format
        // "lain" berujung spawn proses ffprobe) itu I/O blocking. Kalau GIL
        // gak dilepas selama ini, thread Python LAIN (termasuk main/UI
        // thread yang jalanin event loop Qt) ikut nge-freeze selama proses
        // ini berjalan — makanya dibungkus py.allow_threads().
        let file_path_owned = file_path.to_string();

        let (duration, width, height, fps, codec): (f64, u32, u32, f64, String) =
            py.allow_threads(|| -> PyResult<(f64, u32, u32, f64, String)> {
                let path = Path::new(&file_path_owned);
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();

                if ext == "mp4" || ext == "m4v" || ext == "mov" {
                    // Kalau parser mp4 crate-nya gagal (mis. file rusak / brand
                    // aneh yg belum ke-handle), tetap coba ffprobe drpd nyerah.
                    baca_mp4_info(&file_path_owned)
                        .or_else(|_| baca_ffprobe_dinamis(&file_path_owned))
                } else if ext == "mkv" || ext == "webm" {
                    baca_mkv_info(&file_path_owned)
                        .or_else(|_| baca_ffprobe_dinamis(&file_path_owned))
                } else {
                    // ffprobe DI WAKTU JALAN — proses eksternal, blocking penuh.
                    baca_ffprobe_dinamis(&file_path_owned)
                }
            })?;

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
    fn new(py: Python<'_>, file_path: &str) -> PyResult<Self> {
        let info = VideoInfo::new(py, file_path)?;
        Ok(VideoFrameReader {
            path: file_path.to_string(),
            duration: info.duration,
            width: info.width,
            height: info.height,
        })
    }

    /// Lompat ke detik tertentu → kembalikan bingkai
    fn seek_and_read(&mut self, py: Python<'_>, second: f64) -> PyResult<Py<PyArray3<u8>>> {
        use std::process::Command;

        let path_owned = self.path.clone();
        let second_owned = second;

        // py.allow_threads: lepas GIL selama nunggu proses ffmpeg selesai.
        // Tanpa ini, thread lain (termasuk main/UI thread Qt) ikut ke-block
        // selama ffmpeg jalan — inilah penyebab aplikasi "Not Responding"
        // saat hover seekbar / load thumbnail playlist.
        let output = py.allow_threads(|| {
            Command::new("ffmpeg")
                .args(&[
                    "-ss", &format!("{}", second_owned),
                    "-i", &path_owned,
                    "-vframes", "1",
                    "-f", "rawvideo",
                    "-pix_fmt", "rgb24",
                    "-v", "quiet",
                    "-"
                ])
                .output()
        })
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

        let py_arr: Py<PyArray3<u8>> = arr.into_pyarray_bound(py).into();
        Ok(py_arr)
    }
}

// ═══════════════════════════════════════════════
// FUNGSI BANTU: BACA HEADER MP4 (pakai crate `mp4`, bukan parser manual)
// ═══════════════════════════════════════════════
//
// CATATAN: versi sebelumnya parsing box MP4 (moov/mvhd/avc1/dst) manual
// byte-per-byte, dan ada bug alignment di box `mvhd` (skip byte yg salah)
// yg bikin semua box SESUDAHNYA (termasuk avc1/hev1 yg nyimpen width/
// height) ke-parse dari offset yg salah -> width/height sering kebaca 0
// tanpa error apapun. Makanya thumbnail nongol sebagai gambar 0x0 (kosong)
// alih-alih fallback ke OpenCV. Cargo.toml sebenarnya udah nyantumin
// `mp4 = "0.13.0"` sebagai dependency tapi gak pernah dipakai -- sekarang
// dipakai beneran di sini.

fn baca_mp4_info(path_str: &str) -> PyResult<(f64, u32, u32, f64, String)> {
    let f = File::open(path_str)
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("Buka file: {}", e)))?;
    let size = f.metadata()
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("Metadata file: {}", e)))?
        .len();
    let reader = BufReader::new(f);

    let mp4 = mp4::Mp4Reader::read_header(reader, size)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("Parse MP4 gagal: {}", e)))?;

    let duration = mp4.duration().as_secs_f64();

    // Cari track video pertama.
    for track in mp4.tracks().values() {
        if track.track_type().map(|t| t == mp4::TrackType::Video).unwrap_or(false) {
            let width  = track.width() as u32;
            let height = track.height() as u32;
            let fps    = track.frame_rate();
            let codec  = track
                .media_type()
                .map(|m| m.to_string())
                .unwrap_or_else(|_| "Unknown".to_string());
            return Ok((duration, width, height, fps, codec));
        }
    }

    // Gak ada track video (mis. file audio-only) -- tetap kembalikan durasi.
    Ok((duration, 0, 0, 0.0, "Unknown".to_string()))
}

// ═══════════════════════════════════════════════
// FUNGSI BANTU: BACA MKV/WEBM (pakai crate `matroska`)
// ═══════════════════════════════════════════════
//
// Sebelumnya fungsi ini SELALU return Err tanpa pernah nyoba parsing apapun
// (dan VideoInfo::new gak fallback ke ffprobe kalau ini gagal) -- itu bug
// yg dilaporkan sebelumnya. Sekarang beneran parsing pakai crate `matroska`
// yg juga udah ada di Cargo.toml tapi gak kepake.

fn baca_mkv_info(path_str: &str) -> PyResult<(f64, u32, u32, f64, String)> {
    // Di matroska 0.6.0 belum ada fungsi bebas `matroska::open()` (itu baru
    // ditambahin di versi lebih baru) -- yang tersedia adalah method
    // `Matroska::open()` yang nerima apa aja yang implement Read + Seek.
    let f = File::open(path_str)
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(format!("Buka file: {}", e)))?;
    let reader = BufReader::new(f);

    let mkv = matroska::Matroska::open(reader)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("Parse MKV/WebM gagal: {:?}", e)))?;

    let duration = mkv.info.duration.map(|d| d.as_secs_f64()).unwrap_or(0.0);

    for track in mkv.tracks.iter() {
        if track.tracktype == matroska::Tracktype::Video {
            let (width, height) = match &track.settings {
                matroska::Settings::Video(v) => (v.pixel_width as u32, v.pixel_height as u32),
                _ => (0, 0),
            };
            // matroska crate gak ngasih fps langsung -- biarin 0.0, nanti
            // dilengkapi via OpenCV di sisi Python (lihat _PropertiesMetaWorker
            // / ThumbnailGenerator, keduanya udah handle fps<=0 sbg "belum lengkap").
            let codec = if track.codec_id.is_empty() {
                "Unknown".to_string()
            } else {
                track.codec_id.clone()
            };
            return Ok((duration, width, height, 0.0, codec));
        }
    }

    Ok((duration, 0, 0, 0.0, "Unknown".to_string()))
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
