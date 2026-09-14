# M0 真机宿主（Swift）

**先说清楚：`librshell_m0.a` 不是 App，它跑不起来。** 它是一个静态库，
必须被链接进一个 iOS App 目标里才能运行。这个目录就是那个最小的、能跑的目标所需的三样东西。

## 为什么是 Swift 而不是 Flutter

M0 要回答的问题只有一个：**iOS 沙箱里 russh 能不能连出去**。
Flutter 在这一步只会引入第二个未知量（构建集成），让失败无法归因。
而且 Flutter 目前还没装、模拟器 runtime 也没有（见下）。

Rust 产物在两种宿主下是同一个 `.a`，M1 换成 Flutter + rinf 时这部分**零返工**。

## 先跑起飞前检查（不用打开 Xcode）

```bash
./scripts/link-check.sh
```

它会构建 device + simulator 两个切片的 `.a`，用 `clang` 把它们各自链进一个 iOS 可执行文件，
然后审计符号。**实测结论：两个切片都链接成功，唯一的额外链接标志是 `-liconv`；
PTY 家族符号加 `-Wl,-dead_strip` 后全部为 0。**

也就是说，M0b 里唯一不是 GUI 操作的那个未知量（「`.a` 链进 iOS App 会不会缺东西」）
已经被这个脚本回答掉了。剩下就是下面这些点几下的事。

## 手工建工程的步骤（约 15 分钟）

1. Xcode → New Project → iOS → App。Product Name `GuoSSHell`，Interface **SwiftUI**，
   Language **Swift**。删掉自动生成的 `ContentView.swift`，把 `M0Probe.swift` 拖进去。
2. 加 Bridging Header：
   把 `GuoSSHell-Bridging-Header.h` 拖进工程，
   Build Settings → `SWIFT_OBJC_BRIDGING_HEADER` = `GuoSSHell-Bridging-Header.h`。
3. 加构建阶段：
   Build Phases → ＋ → New Run Script Phase，**拖到 Compile Sources 之前**，
   脚本写 `"$SRCROOT/../ios-host/build-rust.sh"`（按你的实际相对路径调整），
   取消勾选 "Based on dependency analysis"。
4. 链接设置（Build Settings）：
   ```
   LIBRARY_SEARCH_PATHS = $(BUILT_PRODUCTS_DIR)
   OTHER_LDFLAGS        = -lrshell_m0 -liconv
   ```
   `-liconv` 不是可选项：rusqlite 的 bundled SQLite 与 ring 在 Apple 平台会引用
   `/usr/lib/libiconv.2.dylib`，`otool -L` 实测可见，Xcode 不会自动带上。

   **`DEAD_CODE_STRIPPING` 保持 `YES`**（Release 默认就是 YES，别去关它）。
   实测：不加 dead strip 时最终可执行文件会导入 `_openpty` / `_login_tty` / `_fork` /
   `_posix_spawnp` 等符号（来自 iOS 上从不调用的本地 PTY 传输，但符号被保留）；
   加上之后**全部为 0**，体积从 13 MB 降到 3.7 MB。这是 App Store 送审前该干净的地方。
5. **选运行目标。** 推荐先选 **My Mac (Designed for iPad)**（Apple Silicon 直接可用，不用下载模拟器 runtime，
   跑的是 iPad 版二进制、走 iOS 沙箱）。要更接近真机就补装 iPad 模拟器 runtime 后选 iPad 模拟器。
   ⚠ 工程里带 "My Mac" 的目标有两个，**别选成 macOS 那个**——那是 macOS 二进制，
   跑通了也证明不了 iOS 的事。
6. 填目标、点按钮，看 transcript。

## Info.plist

只连公网 VPS 的话不需要动。**要连内网地址就必须加**这两项，否则首次连接
`connect()` 会静默超时（不是报错，这是最容易浪费时间的地方）：

```xml
<key>NSLocalNetworkUsageDescription</key>
<string>需要访问本地网络以连接到你的局域网主机</string>
<key>NSBonjourServices</key>
<array>
  <string>_ssh._tcp</string>
</array>
```

`NSBonjourServices` 只有用 Bonjour 发现时才必须；但只要涉及内网访问，
`NSLocalNetworkUsageDescription` 就该在。iOS 没有「主动申请本地网络权限」的 API，
权限只能靠第一次实际访问触发，所以**代码必须把超时当作「可能没授权」来处理**，
并给出「去设置里打开」的引导。

## 模拟器 / "Designed for iPad" 能验什么，不能验什么

它们跑在 macOS 用户态，`fork` / `openpty` 都在。拿它们测「本地 PTY 在 iOS 上不工作」
会得出错误结论（会显示能跑），反过来也会掩盖别的沙箱问题。

**但 M0b 问的不是 PTY，是「iOS App 沙箱里 russh 能不能连出去」。**
这里唯一的平台差异就是 socket 与 App 沙箱，所以模拟器 / "Designed for iPad" 的结论可信。
真机与它们仍有差异（蜂窝网络、后台挂起、内存上限），那些留给 M1 与 M4。

## 环境要求

- macOS（Apple Silicon）+ Xcode 16+。
- **iOS platform 已安装**（没装之前一个 iOS 运行目标都不会有）。
- 运行目标选 **"My Mac (Designed for iPad)"** 或 **iPad 模拟器**，不需要真机。

具体的版本号、工具链路径、已注册设备这类**因机而异**的信息不入库。

