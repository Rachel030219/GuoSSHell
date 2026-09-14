#ifndef GUOSSHELL_M0_BRIDGING_HEADER_H
#define GUOSSHELL_M0_BRIDGING_HEADER_H

#include <stdint.h>

/// 与 Rust 侧 `#[unsafe(no_mangle)] pub extern "C" fn rshell_m0_smoke` 一一对应。
///
/// 返回值是堆分配的 NUL 结尾 UTF-8 JSON：
///   {"ok":true,"text":"outcome=... bytes=... --- transcript --- ..."}
///   {"ok":false,"error":"connect: TransportError { failure: Network }"}
///
/// 调用方必须用 rshell_m0_free() 释放，否则每调一次泄漏一份 transcript。
char *rshell_m0_smoke(const char *host,
                      uint16_t port,
                      const char *username,
                      const char *password,
                      const char *known_hosts_path);

/// M1 前置验证：构造终端引擎、喂一段带颜色的字节、渲染一帧并序列化。
char *rshell_m0_engine_smoke(void);

void rshell_m0_free(char *pointer);

#endif /* GUOSSHELL_M0_BRIDGING_HEADER_H */
