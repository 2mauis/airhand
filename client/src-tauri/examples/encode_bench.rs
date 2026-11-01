use airhand_lib::host::{capture, encoder::Encoder};
use airhand_proto::session::{Codec, QualitySettings};
use std::time::{Duration, Instant};
fn main() {
    let sub = capture::subscribe(0, 60);
    sub.slot().wait_change(0, u64::MAX, Duration::from_secs(3));
    let (_, f) = sub.slot().try_newer(0).expect("capture");
    println!("screen {}x{}", f.width, f.height);
    let mut frames = vec![f.clone()];
    let mut seq = 0;
    let t = Instant::now();
    while frames.len() < 30 && t.elapsed() < Duration::from_secs(3) {
        sub.slot()
            .wait_change(seq, u64::MAX, Duration::from_millis(50));
        if let Some((s, f)) = sub.slot().try_newer(seq) {
            seq = s;
            frames.push(f);
        } else {
            frames.push(frames.last().unwrap().clone());
        }
    }
    for (codec, hw, max_short) in [
        (Codec::H264, false, 1080),
        (Codec::H264, true, 1080),
        (Codec::H265, true, 1080),
        (Codec::AV1, true, 1080),
        (Codec::H264, true, 0),
        (Codec::AV1, true, 0),
    ] {
        let s = QualitySettings {
            codec,
            max_short,
            fps: 60,
            ..Default::default()
        };
        let mut e = Encoder::new(f.width, f.height, &s, 6000, hw).unwrap();
        let t = Instant::now();
        let k = e.encode(&frames[0], true).unwrap().unwrap();
        let key_ms = t.elapsed().as_secs_f64() * 1e3;
        let mut total = 0;
        let mut tt = 0.0;
        let mut n = 0;
        for fr in &frames[1..] {
            let t = Instant::now();
            if let Some(o) = e.encode(fr, false).unwrap() {
                total += o.data.len();
                n += 1;
            }
            tt += t.elapsed().as_secs_f64() * 1e3;
        }
        let i = e.info();
        println!(
            "{:28} {}x{} key {:>7}B {:>6.1}ms | P n={n} avg {:>6}B {:>5.2}ms",
            i.name,
            i.width,
            i.height,
            k.data.len(),
            key_ms,
            total / n.max(1),
            tt / (frames.len() - 1) as f64
        );
    }
}
