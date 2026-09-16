//! `hub` —— rinf 的 Rust 侧入口。
//!
//! * `signals`      rinf 边界上的信号类型（适配层，不重建上游业务模型）
//! * `frame_codec`  RenderFrame → run 压缩字节流（自 bench_frame.rs 提升）
//! * `session`      会话 actor（M0 验证过的装配 + rinf 信号）

mod frame_codec;
mod session;
mod signals;

use rinf::{dart_shutdown, write_interface};
use tokio::spawn;

write_interface!();

// multi_thread 是 M0 验证过的形态（rust/src/lib.rs 的 blocking_smoke 用
// worker_threads=2）。current_thread 下 TOFU 交互回合会饿死（实测：
// TCP 连上后握手无限挂起；换 multi_thread 立即通过）。
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    // 单会话监督者：接 Connect / Resize / Disconnect，逐个转交会话任务。
    spawn(session::supervisor());

    dart_shutdown().await;
}
