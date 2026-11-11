//! 引擎控制队列的容量回归：一轮 `poll` 之内排入动作最多的路径——开启单声道复制与浮点转整数的
//! 曲目，第一次入队就遇到非有限样本，解码器另有若干条结构修正——每个动作都必须送到宿主。
//!
//! 曾经的问题：队列只有 6 格，修正达到 2 条时 `DrainDevice`（或 `TrackSkipped`）被丢弃，引擎
//! 永远停在等待设备排空；达到 3 条时 `Close` 也丢失，音源不关闭。

use qy_core::engine::{
    Capabilities, ChannelPolicy, Engine, EngineConfig, EngineFault, ErrorPolicy, FloatPolicy,
    FrameSink, HostAction, HostEvent, MAX_REPAIRS, PACK_SCRATCH_BYTES, Poll, Repair, Resources,
    ReusePolicy,
};
use qy_core::{
    ChannelLayout, Core, DecodeError, Decoder, DeviceFormat, Endian, INPUT_WINDOW_BYTES, Pcm,
    Progress, SampleEncoding, SourceFormat, Status,
};
use qy_pcm_format::Placement;
use qy_pcm_format::pack::{ChannelMode, Fidelity};

/// 单声道 binary32：读到第一个字节后产出 8 帧，第 4 帧（序号 3）是 NaN；`repairs` 条结构修正。
struct NanDecoder {
    opened: bool,
    pcm: [i32; 8],
    pending: usize,
    repairs: usize,
}

impl NanDecoder {
    fn new(repairs: usize) -> Self {
        let one = 0.5f32.to_bits().cast_signed();
        let nan = f32::NAN.to_bits().cast_signed();
        Self {
            opened: false,
            pcm: [one, one, one, nan, one, one, one, one],
            pending: 0,
            repairs,
        }
    }
}

impl Decoder for NanDecoder {
    fn reset(&mut self) {
        self.opened = false;
        self.pending = 0;
    }

    fn format(&self) -> Option<SourceFormat> {
        self.opened.then(|| {
            SourceFormat::float(48_000, ChannelLayout::MONO, SampleEncoding::Float32).ok()
        })?
    }

    fn decode(&mut self, input: &[u8], eof: bool) -> Result<Progress, DecodeError> {
        if self.opened {
            return Ok(Progress {
                consumed: 0,
                status: Status::End,
            });
        }
        if input.is_empty() {
            if eof {
                return Err(DecodeError::Truncated);
            }
            return Ok(Progress {
                consumed: 0,
                status: Status::NeedInput,
            });
        }
        self.opened = true;
        self.pending = self.pcm.len();
        Ok(Progress {
            consumed: input.len(),
            status: Status::Frames(self.pcm.len()),
        })
    }

    fn pcm(&self) -> &[i32] {
        &self.pcm[self.pcm.len() - self.pending..]
    }

    fn release(&mut self, frames: usize) {
        self.pending = self.pending.saturating_sub(frames);
    }
}

impl Resources for NanDecoder {
    fn repair_count(&self) -> usize {
        self.repairs
    }

    fn repair(&self, index: usize) -> Option<Repair> {
        (index < self.repairs).then_some(Repair::PartialFrameDropped {
            bytes: index as u32 + 1,
        })
    }
}

/// 来者不拒、立即取走的队列。
struct Sink;

impl FrameSink for Sink {
    type Error = ();

    fn push<M: Fidelity>(
        &mut self,
        pcm: &Pcm<'_, M>,
        _: &DeviceFormat,
        _: ChannelMode,
        _: &mut [u8],
    ) -> Result<usize, ()> {
        Ok(pcm.frames())
    }

    fn queued_frames(&self) -> usize {
        0
    }
}

fn leak<T: Clone>(value: T, n: usize) -> &'static mut [T] {
    Box::leak(vec![value; n].into_boxed_slice())
}

/// 播放两首这样的曲目，执行每个动作并回报；返回全部动作、最后一次 `poll` 的结果与内部错误数。
/// 设备拒绝浮点格式、接受 32 位整数立体声（于是走浮点转整数与单声道复制）。
fn run(repairs: usize, policy: ErrorPolicy) -> (Vec<HostAction>, Poll, u32) {
    let Ok(core) = Core::new(leak(0u8, INPUT_WINDOW_BYTES), NanDecoder::new(repairs)) else {
        panic!("核心");
    };
    let cfg = EngineConfig {
        caps: Capabilities {
            max_rate: 192_000,
            max_channels: 2,
            max_bits: 32,
            queue_bytes: 1 << 20,
            prefill_ms: 0,
        },
        reuse: ReusePolicy::Accepts,
        on_error: policy,
    };
    let mut e = Engine::new(core, Sink, leak(0u8, PACK_SCRATCH_BYTES), cfg);
    e.set_float_policy(FloatPolicy::ToInt);
    e.set_channel_policy(ChannelPolicy::DuplicateMono);
    e.set_convert_workspace(leak(0i32, 64));
    e.play(0, 2);
    let Ok(device) = DeviceFormat::new(48_000, 2, 32, 32, Endian::Little, Placement::MsbAligned)
    else {
        panic!("设备格式");
    };
    let mut seen = Vec::new();
    let mut last = Poll::Idle;
    for _ in 0..200 {
        last = e.poll();
        let action = match last {
            Poll::Action(a) => a,
            Poll::Yield => continue,
            Poll::Wait(_) | Poll::Idle => break,
        };
        seen.push(action);
        let handled = match action {
            HostAction::Open { generation, .. } => e.handle(HostEvent::Opened { generation }),
            HostAction::Read {
                generation,
                request,
                ..
            } => {
                let Some(target) = e.read_target(generation, request) else {
                    panic!("读取目标");
                };
                target[0] = 1;
                e.handle(HostEvent::ReadDone {
                    generation,
                    request,
                    len: 1,
                })
            }
            HostAction::OpenDevice { source, .. } if source.encoding().is_float() => {
                e.handle(HostEvent::DeviceOpenFailed)
            }
            HostAction::OpenDevice { .. } => e.handle(HostEvent::DeviceOpened { format: device }),
            HostAction::DrainDevice => e.handle(HostEvent::DeviceDrained),
            HostAction::Failed { .. } | HostAction::Finished => break,
            _ => Ok(()),
        };
        assert_eq!(handled, Ok(()), "{action:?}");
    }
    (seen, last, e.stats().protocol_errors)
}

fn count(seen: &[HostAction], f: impl Fn(&HostAction) -> bool) -> usize {
    seen.iter().filter(|a| f(a)).count()
}

#[test]
fn every_action_of_the_busiest_round_reaches_the_host() {
    for repairs in 0..=MAX_REPAIRS {
        // 出错即停：每个动作各一次，最后报告失败（而不是停在等待设备排空）。
        let (seen, last, errors) = run(repairs, ErrorPolicy::Stop);
        let what = format!("停止策略，{repairs} 条修正：{seen:?}");
        assert_eq!(errors, 0, "{what}");
        let failed = HostAction::Failed {
            track: 0,
            fault: EngineFault::NonFinite { frame: 3 },
        };
        assert_eq!(last, Poll::Action(failed), "{what}");
        for (label, n) in [
            (
                "声道复制",
                count(&seen, |a| {
                    matches!(a, HostAction::ChannelsDuplicated { .. })
                }),
            ),
            (
                "浮点转换",
                count(&seen, |a| matches!(a, HostAction::FloatConverted { .. })),
            ),
            (
                "浮点统计",
                count(&seen, |a| matches!(a, HostAction::FloatStats { .. })),
            ),
            (
                "关闭音源",
                count(&seen, |a| matches!(a, HostAction::Close { .. })),
            ),
            (
                "排空设备",
                count(&seen, |a| matches!(a, HostAction::DrainDevice)),
            ),
        ] {
            assert_eq!(n, 1, "{label}：{what}");
        }
        assert_eq!(
            count(&seen, |a| matches!(a, HostAction::Repaired { .. })),
            repairs,
            "{what}"
        );

        // 出错跳过：第一首报告为已跳过，第二首照常打开，同样在非有限样本处失败。
        let (seen, last, errors) = run(repairs, ErrorPolicy::Skip);
        let what = format!("跳过策略，{repairs} 条修正：{seen:?}");
        assert_eq!(errors, 0, "{what}");
        let skipped = HostAction::TrackSkipped {
            track: 0,
            fault: EngineFault::NonFinite { frame: 3 },
        };
        assert_eq!(count(&seen, |a| *a == skipped), 1, "{what}");
        let failed = HostAction::Failed {
            track: 1,
            fault: EngineFault::NonFinite { frame: 3 },
        };
        assert_eq!(last, Poll::Action(failed), "{what}");
        assert_eq!(
            count(&seen, |a| matches!(a, HostAction::Close { .. })),
            2,
            "{what}"
        );
        assert_eq!(
            count(&seen, |a| matches!(a, HostAction::Repaired { .. })),
            2 * repairs,
            "{what}"
        );
    }
}
