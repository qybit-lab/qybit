# 版本记录

版本规则与对外接口的范围见 [docs/e4-versioning.md](docs/e4-versioning.md)。0.x 阶段次版本号变化表示有破坏性修改，写在该版本的“破坏性”小节。插件仓库按 tag 以 git 依赖（暂不发布 crates.io）。

## 未发布

### 许可

- 许可由 MIT OR Apache-2.0 改为 PolyForm Noncommercial License 1.0.0（`LICENSE`、`NOTICE`）：非商业用途免费，商业用途须取得商业许可并付费（2026-11-11 用户决定）。`v0.1.0` 及更早的提交仍按当时的 MIT OR Apache-2.0。

### 修复

- 引擎：控制队列容量不足（6 格）。开启单声道复制与浮点转整数的曲目遇到非有限样本、解码器又有 2 条以上结构修正时，`DrainDevice` 或 `TrackSkipped` 被丢弃，引擎停在等待设备排空；3 条以上时 `Close` 也丢失。容量按一轮 `poll` 的最坏情况重新确定。官方解码器走不到这条路径，带结构修正的插件解码器可以触发。
- 命令行：文件名不是 UTF-8 时不再 panic（参数改为按系统原样读取）；`qybit decode` 解到一半出错时退出码为 1（此前为 0）。
- Linux：SIGTERM 与 SIGHUP 同 Ctrl+C 一样先停止播放并恢复设备硬件音量（此前进程被直接结束，音量停在播放时的值）。
- FLAC：码流之后的数据（文件末尾的 ID3v1 标签、填充）不再使曲目以 `LostSync` 失败、播放列表停止；声明的总样本数已解满时正常结束并写明（[tasks/W2.md](tasks/W2.md)）。

### 对外接口（只有新增，没有破坏性修改）

- `qybit::run_args_os(&formats, &[OsString])`；`run_args(&[String])` 不变。
- `qy_player::Diagnostics::end_notes`（带默认实现）。
- `FlacDecoder::trailing_data`、`AutoDecoder::trailing_data`。
- 命令行：播放命令中单独的 `--` 之后的参数都按文件处理。

## [0.1.0] - 2026-10-19

第一个版本 tag。此前各 crate 版本为 `0.0.0`，没有兼容承诺。

### 对外接口

- 解码器契约：`qy_core::Decoder`（`decode` / `pcm` / `release` / `seek` / `seek_request` / `take_preroll` / `reset` / `format`）、`Status`、`Progress`、`DecodeError`、`SeekError`；样本为交错、右对齐的 `i32`，携带有效位数（X1、[docs/x1-extensions.md](docs/x1-extensions.md)）。
- 格式：`SourceFormat`（整数、浮点、DSD；有损标记 E1）、`ChannelLayout`、`DeviceFormat` 与 DSD 传输方式（D1）。
- 文件头识别：`qy_core::Identify`、`magic_matches`，官方解码器的 `identify` 与 `AutoDecoder::identify`（E3）。
- 定位预滚：`Decoder::take_preroll`（E5）。
- 解码需要的读取跳转：`Decoder::take_jump`、`JumpGuard` / `Jump` / `JumpFault` / `ParseJump`、`Fault::Jump`、`HostAction::ParseJumped`；嵌入式时序预算 `Load` 增加 `header_jumps` / `unit_jumps` / `jump_us`，`Margins` 增加 `seek_response_us` / `header_jumps`；`Storage::seek` 越过文件末尾时成功、之后读取返回 0（E6）。
- DSD 输出通道：`SampleEncoding::Dsd`、`SourceFormat::dsd`、`DeviceFormat::dsd`，ALSA 原生 DSD 与 DoP、WASAPI DoP（D1）。
- 应用组装：`qy_player::Formats` / `Builtin` / `Probe` / `TrackInfo` / `read_header`，`qybit::run` / `run_args`，`qybit_gui::run`（E2）；`HostResources::with_decoder`、`Diagnostics`（X1）。
- 契约测试：`qy-testkit` 的 `check`、`check_seek`、`check_identify`、`decode`。

### 版本决定（E6）

E6 修改了 M0 T4 起冻结的接口（`Fault` 新增 `Jump` 变体）并给公开的枚举与结构体增加了变体与字段，按 E4 的规则属于破坏性修改。首个版本 tag 当时尚未发布（远端没有任何 tag），用户 2026-10-19 同意把 E6 纳入 0.1.0，并在 E6 完成后再次确认（决定记录见 [tasks/E6.md](tasks/E6.md) §0），所以不升到 0.2.0；`v0.1.0` 打在 E6 完成后的提交 `5e9a9fc` 上。

### 基础格式

WAV（整数、浮点、8 位、EXTENSIBLE、RF64/BW64）、FLAC、AIFF / AIFF-C 未压缩（`NONE`、`twos`、`sowt`、`fl32`/`fl64`），均为自行实现的解码器。
