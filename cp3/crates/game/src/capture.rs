//! 루프백 녹음 (측정용). 게임이 장치로 내보낸 소리를 오인페 내부 루프백으로 되받아 파일로 남긴다.
//! 녹음 스레드는 실시간 경로가 아니므로 파일 쓰기를 직접 한다. 형식은 CP1과 같다:
//! `capture_2ch.f32` (스테레오 f32), `capture_packets.csv` (패킷마다 QPC·장치 위치).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use synchronizer::qpc;
use synchronizer::wasapi::{self, IAudioCaptureClient, IAudioClock, Stream};
use windows::Win32::Media::Audio::AUDCLNT_BUFFERFLAGS_SILENT;
use windows::Win32::System::Threading::WaitForSingleObject;

pub struct CaptureMeta {
    pub name: String,
    pub rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub silent_packets: u64,
}

pub fn capture_loop(name: &str, out: &Path, stop: Arc<AtomicBool>, started: Arc<AtomicBool>) -> Result<CaptureMeta, String> {
    let r = (|| -> Result<CaptureMeta, Box<dyn std::error::Error>> {
        wasapi::com_init()?;
        wasapi::mmcss_pro_audio();
        let ep = wasapi::find(true, name)?;
        let s = Stream::open(&ep.device)?;
        if s.bits != 32 {
            return Err(format!("녹음 믹스 형식이 32비트 float가 아님: {} bit", s.bits).into());
        }
        let cc: IAudioCaptureClient = s.service()?;
        let _clock: IAudioClock = s.service()?;
        let ch = s.channels as usize;
        let mut log = BufWriter::new(File::create(out.join("capture_packets.csv"))?);
        writeln!(log, "qpc,device_pos,qpc100ns,frames,flags")?;
        let mut wav = BufWriter::new(File::create(out.join("capture_2ch.f32"))?);
        let (mut total, mut silent_packets) = (0u64, 0u64);
        unsafe { s.client.Start()? };
        started.store(true, Ordering::Release);
        while !stop.load(Ordering::Acquire) {
            unsafe { WaitForSingleObject(s.event, 2000) };
            loop {
                let n = unsafe { cc.GetNextPacketSize()? };
                if n == 0 {
                    break;
                }
                let now = qpc::now();
                let (mut data, mut frames, mut flags, mut dpos, mut qpos) = (std::ptr::null_mut(), 0u32, 0u32, 0u64, 0u64);
                unsafe { cc.GetBuffer(&mut data, &mut frames, &mut flags, Some(&mut dpos), Some(&mut qpos))? };
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                silent_packets += silent as u64;
                let p = data as *const f32;
                for i in 0..frames as usize {
                    for c in 0..2 {
                        let v = if silent || c >= ch { 0.0 } else { unsafe { *p.add(i * ch + c) } };
                        wav.write_all(&v.to_le_bytes())?;
                    }
                }
                unsafe { cc.ReleaseBuffer(frames)? };
                total += frames as u64;
                writeln!(log, "{now},{dpos},{qpos},{frames},{flags}")?;
            }
        }
        unsafe { s.client.Stop()? };
        log.flush()?;
        wav.flush()?;
        Ok(CaptureMeta { name: ep.name, rate: s.rate, channels: s.channels, frames: total, silent_packets })
    })();
    if r.is_err() {
        started.store(true, Ordering::Release);
    }
    r.map_err(|e| format!("녹음: {e}"))
}
