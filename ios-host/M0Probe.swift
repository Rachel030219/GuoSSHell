// M0 真机验证宿主：一个屏幕，四个输入框，一个按钮。
//
// 这不是 GuoSSHell 的 UI，M1 会被 Flutter + rinf 整体替换。
// 它存在的唯一目的是把「iOS 沙箱里 SSH 通不通」这一个问题单独回答掉。
//
// 用法：在 Xcode 里新建一个 iOS App（Interface: SwiftUI），把本文件替换掉
// ContentView.swift，按 ios-host/README.md 配好 bridging header / 链接参数 /
// 构建阶段脚本，然后选真机运行。

import SwiftUI

@main
struct GuoSSHellM0App: App {
    var body: some Scene {
        WindowGroup { M0ProbeView() }
    }
}

struct M0ProbeView: View {
    @State private var host = ""
    @State private var port = "22"
    @State private var username = ""
    @State private var password = ""
    @State private var transcript = ""
    @State private var status = "未开始"
    @State private var busy = false

    var body: some View {
        NavigationStack {
            Form {
                Section("目标（M0 阶段写死，不进 Keychain）") {
                    TextField("主机（内网 IP 或域名）", text: $host)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                    TextField("端口", text: $port).keyboardType(.numberPad)
                    TextField("用户名", text: $username)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    SecureField("密码", text: $password)
                }

                Section {
                    Button {
                        run()
                    } label: {
                        HStack {
                            Text(busy ? "连接中…" : "开始 SSH 探测")
                            Spacer()
                            if busy { ProgressView() }
                        }
                    }
                    .disabled(busy || host.isEmpty || username.isEmpty)

                    LabeledContent("状态", value: status)
                    LabeledContent("known_hosts") {
                        Text(Self.knownHostsPath())
                            .font(.caption.monospaced())
                            .lineLimit(2)
                    }
                }

                Section("transcript") {
                    ScrollView {
                        Text(transcript.isEmpty ? "（空）" : transcript)
                            .font(.system(.caption, design: .monospaced))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(minHeight: 220)
                }
            }
            .navigationTitle("GuoSSHell M0")
        }
    }

    /// App 沙箱内可写路径。M1 换成 rshell-platform 的 PlatformPaths。
    static func knownHostsPath() -> String {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        return base.appendingPathComponent("known_hosts").path
    }

    private func run() {
        busy = true
        status = "连接中…"
        transcript = ""

        let host = self.host
        let user = self.username
        let secret = self.password
        let node = UInt16(self.port) ?? 22
        let knownHosts = Self.knownHostsPath()

        // 不要占主线程：blocking_smoke 会建一个 tokio 多线程运行时并阻塞到读完。
        DispatchQueue.global(qos: .userInitiated).async {
            let raw = host.withCString { hostPointer in
                user.withCString { userPointer in
                    secret.withCString { secretPointer in
                        knownHosts.withCString { pathPointer in
                            rshell_m0_smoke(hostPointer, node, userPointer, secretPointer, pathPointer)
                        }
                    }
                }
            }

            let payload: String
            if let raw {
                payload = String(cString: raw)
                rshell_m0_free(raw)   // 不释放就是每按一次泄漏一份 transcript
            } else {
                payload = #"{"ok":false,"error":"rshell_m0_smoke 返回了空指针"}"#
            }

            let ok = payload.contains(#""ok":true"#)
            DispatchQueue.main.async {
                transcript = payload
                status = ok ? "SSH 通了" : "失败（看 transcript 里的 failure 分类）"
                busy = false
            }
        }
    }
}
