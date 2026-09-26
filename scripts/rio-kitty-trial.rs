use std::fs::File;
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const CHUNK_SIZE: usize = 4096;

fn base64(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        result.push(BASE64_ALPHABET[((triple >> 18) & 63) as usize] as char);
        result.push(BASE64_ALPHABET[((triple >> 12) & 63) as usize] as char);
        result.push(if chunk.len() > 1 {
            BASE64_ALPHABET[((triple >> 6) & 63) as usize] as char
        } else {
            '='
        });
        result.push(if chunk.len() > 2 {
            BASE64_ALPHABET[(triple & 63) as usize] as char
        } else {
            '='
        });
    }
    result
}

fn placement(
    image_id: u32,
    placement_id: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    columns: u16,
    rows: u16,
) -> String {
    format!(
        "\x1b[3;3H\x1b_Ga=p,i={image_id},p={placement_id},x={x},y={y},w={width},h={height},c={columns},r={rows},C=1,q=0;\x1b\\"
    )
}

fn transmit(
    stdout: &mut impl Write,
    image_id: u32,
    width: u32,
    height: u32,
    rgba: &[u8],
    quiet: bool,
) -> io::Result<usize> {
    let encoded = base64(rgba);
    let chunks = encoded.as_bytes().chunks(CHUNK_SIZE).collect::<Vec<_>>();
    let mut written = 0;
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        let header = if index == 0 {
            format!(
                "\x1b_Ga=t,i={image_id},f=32,s={width},v={height},m={more},q={};",
                if quiet { 2 } else { 0 }
            )
        } else {
            format!("\x1b_Gm={more},q={};", if quiet { 2 } else { 0 })
        };
        stdout.write_all(header.as_bytes())?;
        stdout.write_all(chunk)?;
        stdout.write_all(b"\x1b\\")?;
        written += header.len() + chunk.len() + 2;
    }
    stdout.flush()?;
    Ok(written)
}

fn frame(width: u32, height: u32, sequence: u32) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            rgba.extend_from_slice(&[
                (x + sequence) as u8,
                (y + sequence * 2) as u8,
                ((x + y) / 2) as u8,
                255,
            ]);
        }
    }
    rgba
}

struct RawMode(String);

impl RawMode {
    fn enter() -> io::Result<Self> {
        let original = Command::new("stty")
            .arg("-g")
            .stdin(Stdio::inherit())
            .output()?;
        if !original.status.success() {
            return Err(io::Error::other("stty -g failed; run inside a terminal"));
        }
        let original = String::from_utf8_lossy(&original.stdout).trim().to_owned();
        let status = Command::new("stty").args(["raw", "-echo"]).status()?;
        if !status.success() {
            return Err(io::Error::other("stty raw -echo failed"));
        }
        Ok(Self(original))
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = Command::new("stty").arg(&self.0).status();
    }
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

unsafe extern "C" {
    fn poll(fds: *mut PollFd, nfds: u32, timeout: i32) -> i32;
}

#[derive(Default)]
struct Replies {
    pending: Vec<u8>,
}

impl Replies {
    fn next(&mut self, timeout: Duration) -> io::Result<String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(end) = self.pending.windows(2).position(|bytes| bytes == b"\x1b\\") {
                let response = self.pending.drain(..end + 2).collect::<Vec<_>>();
                return Ok(String::from_utf8_lossy(&response).into_owned());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Kitty reply timed out",
                ));
            }
            let mut fd = PollFd {
                fd: 0,
                events: 1,
                revents: 0,
            };
            let ready = unsafe {
                poll(
                    &mut fd,
                    1,
                    remaining.as_millis().min(i32::MAX as u128) as i32,
                )
            };
            if ready < 0 {
                return Err(io::Error::last_os_error());
            }
            if ready == 0 {
                continue;
            }
            let mut bytes = [0u8; 512];
            let count = io::stdin().read(&mut bytes)?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "terminal closed",
                ));
            }
            self.pending.extend_from_slice(&bytes[..count]);
        }
    }
}

fn acknowledged(reply: &str) -> io::Result<()> {
    if reply.starts_with("\x1b_G") && reply.contains(";OK") {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Kitty rejected command: {reply:?}"
        )))
    }
}

fn acknowledged_for(reply: &str, image_id: u32, placement_id: u32) -> io::Result<()> {
    acknowledged(reply)?;
    let expected = format!("i={image_id},p={placement_id};OK");
    if reply.contains(&expected) {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "unexpected Kitty placement reply: {reply:?}"
        )))
    }
}

fn summary_ms(values: &[Duration]) -> (f64, f64, f64) {
    let mut sorted = values
        .iter()
        .map(|value| value.as_secs_f64() * 1000.0)
        .collect::<Vec<_>>();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: f64| sorted[((sorted.len() - 1) as f64 * p).ceil() as usize];
    (percentile(0.5), percentile(0.95), *sorted.last().unwrap())
}

fn run(log: &mut File) -> io::Result<()> {
    let _raw = RawMode::enter()?;
    let mut stdout = io::stdout().lock();
    let mut replies = Replies::default();
    stdout.write_all(b"\x1b[2J\x1b[H")?;
    transmit(&mut stdout, 42, 800, 600, &frame(800, 600, 0), false)?;
    let upload_reply = replies.next(Duration::from_secs(5))?;
    acknowledged(&upload_reply)?;
    writeln!(log, "static upload: {upload_reply:?}")?;
    log.flush()?;

    let mut crop_times = Vec::new();
    for index in 0..60 {
        let x = index * 6;
        let crop = placement(42, 7, x, 80, 400, 300, 60, 20);
        let started = Instant::now();
        stdout.write_all(crop.as_bytes())?;
        stdout.flush()?;
        acknowledged_for(&replies.next(Duration::from_secs(3))?, 42, 7)?;
        crop_times.push(started.elapsed());
    }
    let (median, p95, max) = summary_ms(&crop_times);
    writeln!(
        log,
        "crop replacements: 60/60 ACK, median={median:.2}ms p95={p95:.2}ms max={max:.2}ms"
    )?;
    log.flush()?;

    let period = Duration::from_micros(1_000_000 / 15);
    let started = Instant::now();
    let mut frame_times = Vec::new();
    let mut late = 0;
    let mut total_wire_bytes = 0usize;
    for sequence in 0..450u32 {
        let due = started + period * sequence;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        } else if Instant::now() > due + period {
            late += 1;
        }
        let sent_at = Instant::now();
        total_wire_bytes += transmit(&mut stdout, 43, 320, 180, &frame(320, 180, sequence), true)?;
        let command = placement(43, 8, 0, 0, 320, 180, 60, 20);
        stdout.write_all(command.as_bytes())?;
        stdout.flush()?;
        acknowledged_for(&replies.next(Duration::from_secs(3))?, 43, 8)?;
        frame_times.push(sent_at.elapsed());
        if sequence % 75 == 74 {
            writeln!(log, "video progress: {}/450 ACK", sequence + 1)?;
            log.flush()?;
        }
    }
    let elapsed = started.elapsed();
    let (median, p95, max) = summary_ms(&frame_times);
    writeln!(
        log,
        "video: 450/450 ACK in {:.2}s, late={} median={median:.2}ms p95={p95:.2}ms max={max:.2}ms, wire={total_wire_bytes} bytes",
        elapsed.as_secs_f64(),
        late
    )?;
    log.flush()?;
    Ok(())
}

fn main() -> io::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| io::Error::other("usage: rio-kitty-trial LOG_PATH"))?;
    let mut log = File::create(path)?;
    if let Err(error) = run(&mut log) {
        writeln!(log, "trial failed: {error}")?;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encodes_raw_rgba_without_padding_errors() {
        assert_eq!(base64(&[0, 0, 0, 255]), "AAAA/w==");
        assert_eq!(base64(&[1, 2, 3]), "AQID");
    }

    #[test]
    fn placement_reuses_image_and_placement_ids_with_a_new_crop() {
        let first = placement(42, 7, 0, 0, 320, 180, 60, 20);
        let second = placement(42, 7, 20, 10, 160, 90, 60, 20);
        assert!(first.contains("a=p,i=42,p=7,x=0,y=0,w=320,h=180,c=60,r=20"));
        assert!(second.contains("a=p,i=42,p=7,x=20,y=10,w=160,h=90,c=60,r=20"));
        assert!(!second.contains("f=32"));
    }

    #[test]
    fn an_acknowledgement_must_match_the_expected_placement() {
        assert!(acknowledged_for("\x1b_Gi=42,p=7;OK\x1b\\", 42, 7).is_ok());
        assert!(acknowledged_for("\x1b_Gi=42,p=8;OK\x1b\\", 42, 7).is_err());
        assert!(acknowledged_for("\x1b_Gi=42,p=7;EINVAL\x1b\\", 42, 7).is_err());
    }
}
