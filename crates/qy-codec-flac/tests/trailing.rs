//! 码流之后的数据（文件末尾附加的 ID3v1 标签、填充等）：STREAMINFO 声明的总样本数（RFC 9639
//! §8.2）已解满、其后的字节解不出帧时，曲目正常结束并记下（[`FlacDecoder::trailing_data`]），
//! 样本与不带尾部数据的同一文件逐个相同；意图不明的情况照旧报错。
//!
//! 文件由真实夹具（libFLAC 编码）加上本测试附加的字节组成。

use std::path::Path;

use qy_codec_flac::{Capacity, FlacDecoder, FlacError};
use qy_core::{DecodeError, Decoder, Status};

const BASE: &str = "cli_16_stereo_44k_default.flac";

fn leak<T: Copy + Default>(n: usize) -> &'static mut [T] {
    Box::leak(vec![T::default(); n].into_boxed_slice())
}

fn fixture(name: &str) -> Vec<u8> {
    match std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    ) {
        Ok(b) => b,
        Err(e) => panic!("{name}: {e}"),
    }
}

fn dec(cap: Capacity) -> FlacDecoder {
    match FlacDecoder::new(cap, leak(cap.workspace_len()), leak(cap.pcm_len())) {
        Ok(d) => d,
        Err(e) => panic!("{e:?}"),
    }
}

/// 按 `chunk` 字节一次从源读入（像核心的输入窗口那样保留未消耗的字节），解码到结束或出错。
/// 返回结果与全部样本。
fn decode(d: &mut FlacDecoder, data: &[u8], chunk: usize) -> (Result<(), DecodeError>, Vec<i32>) {
    let mut samples = Vec::new();
    let mut window: Vec<u8> = Vec::new();
    let mut read = 0;
    loop {
        let eof = read == data.len();
        match d.decode(&window, eof) {
            Err(e) => return (Err(e), samples),
            Ok(p) => {
                window.drain(..p.consumed);
                match p.status {
                    Status::End => return (Ok(()), samples),
                    Status::NeedInput => {
                        assert!(!eof, "源结束后仍要输入");
                        let n = chunk.min(data.len() - read);
                        window.extend_from_slice(&data[read..read + n]);
                        read += n;
                    }
                    Status::Frames(_) | Status::NeedOutput => {
                        samples.extend_from_slice(d.pcm());
                        let ch = d.format().map_or(1, |f| usize::from(f.channels()));
                        d.release(d.pcm().len() / ch);
                    }
                }
            }
        }
    }
}

/// STREAMINFO 的总样本数字段（36 位，§8.2）改为 `total`；夹具的 STREAMINFO 是第一个元数据块。
fn set_total(data: &mut [u8], total: u64) {
    let info = &mut data[8..42];
    info[13] = (info[13] & 0xF0) | ((total >> 32) as u8 & 0x0F);
    info[14..18].copy_from_slice(&(total as u32).to_be_bytes());
}

fn id3v1() -> Vec<u8> {
    let mut tag = b"TAG".to_vec();
    tag.extend_from_slice(&[b' '; 125]);
    tag
}

/// 末尾附加的各种非帧数据：样本与原文件逐个相同，正常结束并记下；与输入分块无关。
#[test]
fn trailing_data_after_a_complete_stream_ends_normally() {
    let plain = fixture(BASE);
    let mut d = dec(Capacity::DESKTOP);
    let (r, reference) = decode(&mut d, &plain, usize::MAX);
    assert_eq!(r, Ok(()));
    assert!(!reference.is_empty());
    assert!(!d.trailing_data(), "没有尾部数据的文件不应标记");

    // 以帧同步码开头、其后不成帧的字节（帧头 CRC-8 不符）。
    let mut fake_sync = vec![0xFF, 0xF8];
    fake_sync.extend_from_slice(&[0x55; 40]);
    let tails: [(&str, Vec<u8>); 5] = [
        ("ID3v1 标签", id3v1()),
        ("零填充", vec![0; 64]),
        ("单个字节", vec![0x00]),
        ("同步码的前半", vec![0xFF]),
        ("伪同步码", fake_sync),
    ];
    for (what, tail) in &tails {
        let mut file = plain.clone();
        file.extend_from_slice(tail);
        for chunk in [usize::MAX, 1, 7, 4096] {
            d.reset();
            let (r, samples) = decode(&mut d, &file, chunk);
            assert_eq!(r, Ok(()), "{what}，每次 {chunk} 字节：{:?}", d.last_error());
            assert!(samples == reference, "{what}，每次 {chunk} 字节：样本不同");
            assert!(d.trailing_data(), "{what}，每次 {chunk} 字节：应标记");
            assert_eq!(d.last_error(), None, "{what}");
        }
    }
    // 复位后标记清除。
    d.reset();
    assert!(!d.trailing_data());
}

/// 意图不明的情况照旧报错：总样本数未知（0）时其后的非帧数据仍是码流错误；声明的总样本数
/// 之后还有有效的帧（实际样本多于声明）仍是总数不符。
#[test]
fn ambiguous_cases_are_still_errors() {
    let plain = fixture(BASE);

    let mut unknown = plain.clone();
    set_total(&mut unknown, 0);
    unknown.extend_from_slice(&id3v1());
    let mut d = dec(Capacity::DESKTOP);
    let (r, _) = decode(&mut d, &unknown, usize::MAX);
    assert_eq!(r, Err(DecodeError::Malformed));
    assert_eq!(d.last_error(), Some(FlacError::LostSync));
    assert!(!d.trailing_data());

    // 第一帧的块大小：把声明的总样本数改成它，其后的有效帧使实际样本多于声明。
    d.reset();
    let first_block = match d.decode(&plain, true) {
        Ok(p) => match p.status {
            Status::Frames(n) => n as u64,
            other => panic!("第一次调用应产出第一帧：{other:?}"),
        },
        Err(e) => panic!("{e:?}"),
    };
    let mut short = plain.clone();
    set_total(&mut short, first_block);
    d.reset();
    let (r, _) = decode(&mut d, &short, usize::MAX);
    assert_eq!(r, Err(DecodeError::Malformed));
    assert_eq!(d.last_error(), Some(FlacError::TotalSamplesMismatch));
    assert!(!d.trailing_data());

    // 码流未完整（总样本数未解满）时的非帧数据仍是码流错误：在最后一帧之前插入标签。
    let mut cut = plain.clone();
    let keep = cut.len() / 2;
    cut.truncate(keep);
    cut.extend_from_slice(&id3v1());
    d.reset();
    let (r, _) = decode(&mut d, &cut, usize::MAX);
    assert!(r.is_err(), "未完整的码流不得按尾部数据结束");
    assert!(!d.trailing_data());
}

/// 第一帧在文件中的位置（`fLaC` 与各元数据块之后）。
fn frames_start(data: &[u8]) -> usize {
    let mut i = 4;
    loop {
        let last = data[i] & 0x80 != 0;
        let len = u32::from_be_bytes([0, data[i + 1], data[i + 2], data[i + 3]]) as usize;
        i += 4 + len;
        if last {
            return i;
        }
    }
}

/// 源结束或输入窗口装满时仍凑不成一帧的尾部数据（以真实的帧头开头）：码流已按声明完整，同样
/// 正常结束。
#[test]
fn incomplete_frame_after_a_complete_stream() {
    let plain = fixture(BASE);
    let start = frames_start(&plain);
    let mut d = dec(Capacity::DESKTOP);
    let (r, reference) = decode(&mut d, &plain, usize::MAX);
    assert_eq!(r, Ok(()));

    // 源结束：末尾是一帧的前 100 字节（帧头合法，数据不全）。
    let mut file = plain.clone();
    file.extend_from_slice(&plain[start..start + 100]);
    for chunk in [usize::MAX, 1, 333] {
        d.reset();
        let (r, samples) = decode(&mut d, &file, chunk);
        assert_eq!(r, Ok(()), "每次 {chunk} 字节：{:?}", d.last_error());
        assert!(samples == reference, "每次 {chunk} 字节：样本不同");
        assert!(d.trailing_data());
    }

    // 窗口装满：各帧的字节长度与块大小（第一帧的长度不含元数据）。
    d.reset();
    let mut frames = Vec::new();
    let mut pos = 0;
    loop {
        match d.decode(&plain[pos..], true) {
            Ok(p) => {
                pos += p.consumed;
                match p.status {
                    Status::End => break,
                    Status::Frames(n) => {
                        let len = if frames.is_empty() {
                            p.consumed - start
                        } else {
                            p.consumed
                        };
                        frames.push((len, n as u64));
                        d.release(n);
                    }
                    other => panic!("{other:?}"),
                }
            }
            Err(e) => panic!("{e:?}"),
        }
    }
    // 找一帧，它比之前所有帧都长：窗口取之前各帧的最大长度，声明的总样本数取到它之前为止。
    let Some(k) = (1..frames.len()).find(|&k| frames[..k].iter().all(|f| f.0 < frames[k].0)) else {
        panic!("夹具中没有比之前各帧都长的帧：{frames:?}");
    };
    let window = frames[..k].iter().map(|f| f.0).max().unwrap_or(0);
    let total: u64 = frames[..k].iter().map(|f| f.1).sum();
    // STREAMINFO 的最大帧长度（§8.2，24 位）改为未声明（0），否则文件头阶段就按声明值拒绝。
    let mut undeclared = plain.clone();
    undeclared[8 + 7..8 + 10].fill(0);
    let mut short = undeclared.clone();
    set_total(&mut short, total);
    let cap = Capacity {
        input_window: window,
        ..Capacity::DESKTOP
    };
    let mut small = dec(cap);
    let (r, samples) = decode(&mut small, &short, usize::MAX);
    assert_eq!(r, Ok(()), "{:?}", small.last_error());
    assert_eq!(samples.len() as u64, total * 2);
    assert!(samples == reference[..samples.len()]);
    assert!(small.trailing_data());
    // 对照：声明的总样本数未解满时，同一帧超出窗口仍按容量不足报告。
    let mut small = dec(cap);
    let (r, _) = decode(&mut small, &undeclared, usize::MAX);
    assert_eq!(r, Err(DecodeError::CapacityExceeded));
    assert_eq!(small.last_error(), Some(FlacError::FrameTooLarge));
    assert!(!small.trailing_data());
}
