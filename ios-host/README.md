# M0 真机宿主（Swift）

**先说清楚：`librshell_m0.a` 不是 App，它跑不起来。** 它是一个静态库，  
必须被链接进一个 iOS App 目标里才能运行。这个目录就是那个最小的、能跑的目标所需的三样东西。

## 为什么是 Swift 而不是 Flutter

M0 要回答的问题只有一个：**iOS 沙箱里 russh 能不能连出去**。  
Flutter 在这一步只会引入第二个未知量（构建集成），让失败无法归因。

Rust 产物在两种宿主下是同一个 `.a`，M1 换成 Flutter + rinf 时这部分**零返工**。

## 先跑起飞前检查（不用打开 Xcode）

```bash
../scripts/link-check.sh
```

它会构建 device + simulator 两个切片的 `.a`，用 `clang` 把它们各自链进一个 iOS 可执行文件，  
然后审计符号。**实测结论：两个切片都链接成功，唯一的额外链接标志是 `-liconv`；  
PTY 家族符号加 `-Wl,-dead_strip` 后全部为 0。**

也就是说，M0b 里唯一不是 GUI 操作的那个未知量（「`.a` 链进 iOS App 会不会缺东西」）  
已经被这个脚本回答掉了。

---

# Xcode 16 的四个坑（全是实测踩出来的）

## 坑 1（最先撞上）：iOS platform 根本没装

Xcode 16 把**平台支持**从 Xcode.app 里拆了出去。**SDK 随 Xcode 装，但能被选中的  
destination 要另外下。** 没下之前 `-showdestinations` 就是这个样子：

```
$ xcodebuild -showdestinations -project GuoSSHell.xcodeproj -scheme GuoSSHell
    Available destinations:
        { platform:macOS, arch:arm64, name:My Mac }
        { platform:macOS, name:Any Mac }
    Ineligible destinations:
        { platform:iOS, name:Any iOS Device,
          error:iOS 18.2 is not installed. To use with Xcode, first download and install the platform }
```

极易误判的地方：`xcodebuild -showsdks` **能看到** `iOS 18.2`，  
`xcrun --sdk iphoneos --show-sdk-path` **也有有效路径** —— 但那是 SDK，不是 platform。  
`xcrun simctl list runtimes` 为空是同一个原因。

```bash
xcodebuild -downloadPlatform iOS     # 约 7–8 GB，含模拟器 runtime
```

或 Xcode → Settings → Components → iOS 18.2 → Get。

**没装之前，运行目标下拉里一个 iOS 选项都不会有**，包括 "My Mac (Designed for iPad)"。  
这不是可跳过的步骤。

## 坑 2：工程模板别选 "Multiplatform"

`New Project` 里选 **iOS → App**。如果选了 **Multiplatform → App**，生成的设置是：

```
SDKROOT           = auto
SUPPORTED_PLATFORMS = "iphoneos iphonesimulator macosx xros xrsimulator"
TARGETED_DEVICE_FAMILY = "1,2,7"
```

一个 target 同时声明支持 iOS / macOS / visionOS，于是 Xcode 在 Mac 上**直接编成原生 macOS  
App**，运行目标里只有 "My Mac"，**永远不会有 "My Mac (Designed for iPad)"**。

原因：**「Designed for iPad」的前提正是 target 只支持 iOS**，让 macOS 去代跑 iOS 二进制。  
两个 identity 互斥 —— `SUPPORTED_PLATFORMS` 里只要有 `macosx`，这条路就不会出现。

要改的话：target → General → **Supported Destinations** → 删掉 `Mac` 和 `Apple Vision`，  
再点 `+` 选 **Mac (Designed for iPad)**。Xcode 会自动把上面三个设置改回去。

## 坑 3：Run Script 用的是 bash **3.2**

Xcode 执行 Run Script 时用的是 macOS 自带的 `/bin/bash`（**3.2.57**），  
不是 Homebrew 的 bash 5，也不是你的交互 shell。所以脚本里**不能出现 bash 4+ 语法**：

| 写法                             | bash 3.2 下的表现              |
| ------------------------------ | -------------------------- |
| `"${arr[@]}"`（数组为空 + `set -u`） | `arr[@]: unbound variable` |
| `"${var,,}"`（小写展开）             | `bad substitution`         |

`ios-host/build-rust.sh` 一开始两条都犯了，Debug 构建必挂（因为 `CARGO_FLAGS` 是空的）。  
现在改成标量 + `tr`，已修。**以后往 Run Script 里加东西，记得`/bin/bash -n` 过一遍。**

顺带一提：Xcode 给这个 phase 的默认 `shellPath` 就是 `/bin/sh`，而 macOS 的 `/bin/sh`  
正是 bash 3.2 —— 这不是巧合，是同一个坑的两种说法。

## 坑 4：User Script Sandboxing 会拦住脚本往构建目录写文件

Xcode 15+ 新建工程默认 `ENABLE_USER_SCRIPT_SANDBOXING = YES`，把 Run Script 关进沙箱。  
Xcode 为它生成的规则里有这么一条：

```
(deny file-read* file-write* (subpath (param "CONFIGURATION_BUILD_DIR")) ...)
```

`CONFIGURATION_BUILD_DIR` 就是 `$(BUILT_PRODUCTS_DIR)` —— 而脚本要往那儿写  
`librshell_m0.a`，于是报：

```
Sandbox: cp(1282) deny(1) file-write-create
         .../DerivedData/GuoSSHell-xxxx/Build/Products/Debug-iphoneos/librshell_m0.a
```

**修法：给这个 Run Script 声明 Output File。**

线索就藏在那份沙箱文件的最后一行注释里：

```
;; Allow read+write for declared and resolved (flattened) outputs
```

**没声明输出时，这行下面就是空的** —— 没有任何放行规则。声明之后 Xcode 会补上：

```
(allow file-read* file-write* (literal (param "SCRIPT_OUTPUT_FILE_0")))
```

`literal` 比 `subpath` 更具体，因此覆盖上面那条 deny。  
（Xcode 自己就用同样的手法：先 `deny file-read* file-write* (subpath (param "SRCROOT"))`，  
再 `allow file-read* (literal (param "SRCROOT")))` 把它盖回来。）

**操作**：Build Phases → Run Script → 展开 **Output Files** → `+` → 填

```
$(BUILT_PRODUCTS_DIR)/librshell_m0.a
```

"Based on dependency analysis" 保持**不勾选**（脚本每次都要跑，增量由 cargo 自己判断）。

> 备选：Build Settings → **User Script Sandboxing** → `NO`。  
> 一行搞定，代价是放弃 Xcode 这层保护。能声明输出就别关它。

---

## 手工建工程的步骤

1. Xcode → New Project → **iOS** → App（**别选 Multiplatform**，见坑 2）。  
   Product Name `GuoSSHell`，Interface **SwiftUI**，Language **Swift**。
2. **删掉模板生成的两个文件**，把 `M0Probe.swift` 拖进来：
   ```
   GuoSSHellApp.swift     ← 模板的 App 入口
   ContentView.swift      ← 模板的首屏
   ```
   `M0Probe.swift` 自带 `@main struct GuoSSHellM0App: App`，本身就是完整的 App 入口。  
   模板那个不删的话：一是**两个 `@main` 打架**，二是它还引用着已被删掉的 `ContentView`，  
   编译必挂。
3. 加 Bridging Header：把 `GuoSSHell-Bridging-Header.h` 拖进工程，设置  
   `SWIFT_OBJC_BRIDGING_HEADER`。

   ⚠ **值必须是相对 `SRCROOT` 的整条路径。** 源码放在 `GuoSSHell/` 子目录里就写  
   `GuoSSHell/GuoSSHell-Bridging-Header.h`；只写文件名会在编译时报
   ```
   error: Build input file cannot be found: '.../GuoSSHell-Bridging-Header.h'
   ```
4. 加构建阶段：Build Phases → ＋ → New Run Script Phase，**拖到 Compile Sources 之前**，  
   脚本内容填 `ios-host/build-rust.sh` 的**绝对路径**：
   ```
   /path/to/GuoSSHell/ios-host/build-rust.sh
   ```
   （Xcode 工程与 Rust 仓库不一定放在同一棵树里 —— 放哪儿由你决定。正因为两者  
   位置无关，用绝对路径最省事；`$(SRCROOT)` 这类相对路径在这里不适用。）

   然后在这个 phase 里：
   - 取消勾选 **"Based on dependency analysis"**（脚本每次都要跑，增量交给 cargo 判断）
   - 展开 **Output Files** → `+` → 填 `$(BUILT_PRODUCTS_DIR)/librshell_m0.a`  
     ⚠ **不填就会被沙箱拦住 cp**，报 `Sandbox: cp(...) deny(1) file-write-create`，见坑 4
5. 链接设置（Build Settings）：
   ```
   LIBRARY_SEARCH_PATHS = $(BUILT_PRODUCTS_DIR)
   OTHER_LDFLAGS        = -lrshell_m0 -liconv
   ```
   ⚠ **`Other Linker Flags` 必须是两个独立的值，别带引号。** Xcode 若把它存成一个带  
   引号的字符串，ld 会把 `-lrshell_m0 -liconv` 当成**一个**库名：
   ```
   ld: library 'rshell_m0 -liconv' not found
   ```
   在 UI 里分成两行填 `-lrshell_m0` 和 `-liconv` 即可。pbxproj 里正确的形态是  
   `OTHER_LDFLAGS = "-lrshell_m0 -liconv";`，错误形态多一层转义引号：  
   `OTHER_LDFLAGS = "\"-lrshell_m0 -liconv\"";`。

   `-liconv` 不是可选项：rusqlite 的 bundled SQLite 与 ring 在 Apple 平台会引用  
   `/usr/lib/libiconv.2.dylib`，`otool -L` 实测可见，Xcode 不会自动带上。

   **`DEAD_CODE_STRIPPING` 保持 `YES`**（Xcode 默认就是 YES，别去关它）。  
   实测：不加 dead strip 时最终可执行文件会导入 `_openpty` / `_login_tty` / `_fork` /  
   `_posix_spawnp` 等符号（来自 iOS 上从不调用的本地 PTY 传输，但符号被保留）；  
   加上之后**全部为 0**，体积从 13 MB 降到 3.7 MB。
6. **选运行目标**（前提是坑 1 的 platform 已经装好）：
   - **My Mac (Designed for iPad)** —— 跑的是 iPad 版二进制、走 iOS 沙箱，不需要真机。
   - **iPad 模拟器** —— 更接近真机。  
     ⚠ 下拉里带 "My Mac" 的项有好几个，**别选成 macOS 那个**（也没有 Catalyst）——  
     那是 macOS 二进制，跑通了也证明不了 iOS 的事。
7. 填目标、点按钮，看 transcript。

## 本地网络权限（要连内网就必须有）

只连公网 VPS 的话不用管。**要连内网地址就必须声明本地网络用途**，否则首次连接  
`connect()` 会**静默超时**——不报错，这是最容易浪费时间的地方。

Xcode 16 的新建工程默认 `GENERATE_INFOPLIST_FILE = YES`，**工程里的 `Info.plist` 往往是空的**，  
所以别去改 XML，直接在 **Build Settings** 里加一项：

```
INFOPLIST_KEY_NSLocalNetworkUsageDescription = 需要访问本地网络以连接到你的局域网主机
```

Debug / Release 两个 configuration 都要加。想确认它真的进了 plist，可以从构建产物里  
把 `Info.plist` 读回来（`plutil -p <App>.app/Info.plist`）看这个 key 在不在。

**不需要** `NSBonjourServices`：那一项只有在用 Bonjour **发现**主机时才要求，  
而本项目的连接方式是手工填地址、不做自动发现，所以不加。

iOS 没有「主动申请本地网络权限」的 API，权限只靠第一次真实访问触发，所以  
**代码必须把超时当作「可能没授权」来处理**，并给出「去设置里打开」的引导。

## 模拟器 / "Designed for iPad" 能验什么，不能验什么

它们跑在 macOS 用户态，`fork` / `openpty` 都在。拿它们测「本地 PTY 在 iOS 上不工作」  
会得出错误结论（会显示能跑），反过来也会掩盖别的沙箱问题。

**但 M0b 问的不是 PTY，是「iOS App 沙箱里 russh 能不能连出去」。**  
这里唯一的平台差异就是 socket 与 App 沙箱，所以模拟器 / "Designed for iPad" 的结论可信。  
真机与它们仍有差异（蜂窝网络、后台挂起、内存上限），那些留给 M1 与 M4。

## 环境要求

- macOS（Apple Silicon）+ Xcode 16+。
- **iOS platform 已安装**（坑 1：`xcodebuild -downloadPlatform iOS`）；没装之前一个 iOS  
  运行目标都不会有。
- 运行目标选 **"My Mac (Designed for iPad)"** 或 **iPad 模拟器**，不需要真机。

具体的版本号、工具链路径、已注册设备这类**因机而异**的信息不入库。  
需要时以 `PLAN.md` §7「环境要求」和 `scripts/setup.sh` 的探测输出为准。
