/*
 * M0 静态库的「命令行链接测试」。
 *
 * 目的：在不打开 Xcode 的前提下，证明 librshell_m0.a 能被一个 iOS 目标完整链接，
 * 并且暴露出除了 -liconv 之外还需要哪些链接标志 / 框架。
 *
 * 注意：这个程序**不打算运行**（它是 iOS 二进制，跑不进 macOS 终端）。
 * 我们只要链接这一步的结论。
 */
#include <stdio.h>

extern char *rshell_m0_smoke(const char *host, unsigned short port,
                             const char *username, const char *password,
                             const char *known_hosts_path);
extern char *rshell_m0_engine_smoke(void);
extern void rshell_m0_free(char *pointer);

int main(void) {
  char *engine = rshell_m0_engine_smoke();
  char *ssh = rshell_m0_smoke("127.0.0.1", 22, "user", "pass", "/tmp/known_hosts");
  printf("%s\n%s\n", engine ? engine : "(null)", ssh ? ssh : "(null)");
  rshell_m0_free(engine);
  rshell_m0_free(ssh);
  return 0;
}
