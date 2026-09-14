//! M0a：在 macOS 上用同一个 Rust 函数直接跑 SSH，先用最少的变量把逻辑与凭证验证掉。
//!
//! ```text
//! cargo run --example m0 -- <host> <port> <user> <password> [known_hosts_path]
//! ```
//!
//! 这一步不需要 Xcode、不需要真机。它验证的是 SSH 本身：
//! 连接、主机密钥 TOFU 落盘、密码认证、远端 PTY、远端 shell、字节回流。
//! 之后 M0b 只是在 iOS 宿主里调同一个函数，唯一剩下的未知量就只有沙箱。

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("usage: m0 <host> <port> <user> <password> [known_hosts_path]");
        std::process::exit(2);
    }
    let port: u16 = args[1].parse().expect("port must be a u16");
    let known_hosts = args
        .get(4)
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir().join("rshell-m0-known_hosts").display().to_string());

    eprintln!(
        "target={}:{} user={} known_hosts={known_hosts}",
        args[0], port, args[2]
    );

    match rshell_m0::blocking_smoke(&args[0], port, &args[2], &args[3], &known_hosts) {
        Ok(text) => println!("{text}"),
        Err(error) => {
            eprintln!("FAILED: {error}");
            std::process::exit(1);
        }
    }
}
