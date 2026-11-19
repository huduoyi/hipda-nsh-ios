# HiPDA·nsh iOS 侧载安装指南（免费 Apple ID）

这份指南配合 `.github/workflows/build-ios.yml` 使用：
**GitHub 云端负责编译，你的 Windows 电脑负责免费签名安装到 iPhone。**

原理：NSH 官方支持 iOS 15+，所以 iOS 18 完全没问题。真机 IPA 必须有签名才能装，
但付费开发者账号（$99/年）不是必须的——用**免费 Apple ID + Sideloadly** 就能签名，
代价是**证书 7 天过期，需要每周重签一次**。

---

## 一、把 workflow 放进你的 GitHub 仓库

代码已经在 Gitee 上，你又已有 GitHub 仓库。两种情况：

**情况 A：GitHub 仓库已经是这份代码的副本**
把本目录里的 `.github/workflows/build-ios.yml` 复制到你仓库的相同路径，提交推送即可。

**情况 B：GitHub 仓库是空的 / 想重新推一份**
在本文件夹（`hipda-nsh`）打开终端，执行：

```bash
git remote remove origin
git remote add origin https://github.com/你的用户名/你的仓库名.git
git branch -M main
git push -u origin main
```

> 注意：Gitee 上的 `dist/` 里有一个 ~几十 MB 的 Windows 安装包，`.gitignore` 已忽略，
> 不影响本仓库。推送前确认 `.github/workflows/build-ios.yml` 在待提交文件里。

---

## 二、触发编译并下载 IPA

1. 打开你的 GitHub 仓库 → 顶部 **Actions** 标签。
2. 左侧选中 **Build iOS IPA (unsigned, for sideload)** → 右侧 **Run workflow** → 绿色按钮 **Run workflow**。
3. 等 3–8 分钟（首次会慢一些），任务变成绿色对勾即成功。
   - 如果失败，点进去看日志；最常见原因见文末「排错」。
4. 在这个成功运行的页面底部，**Artifacts** 区找到 **`HiPDA-nsh-iOS-unsigned`**，点击下载。
5. 下载得到的是一个 zip，解压后是 **`HiPDA-nsh-unsigned.ipa`** —— 这就是待签名的包。

> 免费账号每月有 2000 分钟 CI 额度，macOS 按 10 倍消耗。workflow 设成了**只手动触发**、不随 push 自动跑，就是为了省额度。

---

## 三、Windows 上用 Sideloadly 签名安装

### 3.1 准备

1. **安装 iTunes（桌面版）**：Sideloadly 在 Windows 上依赖它提供的 Apple 设备驱动。
   - 微软商店装的 iTunes 不行，要去 apple.com 下载 `.exe` 桌面版，或安装 `Apple Mobile Device Support`。
2. **下载 Sideloadly**：sideloadly.io → 下载 Windows 版（`.exe`，选带 "Sideloadly" 字样的官方站）。
3. 用数据线把 iPhone 连到电脑，iPhone 上弹「信任此电脑」时点**信任**，输入锁屏密码。
4. iPhone 开启**开发者模式**：设置 → 隐私与安全性 → 开发者模式 → 打开并重启。
   （iOS 16 及以上必须开，否则装上的 App 打不开。）

### 3.2 签名安装

> 先纠正一个常见误解：**Sideloadly 主界面上并没有「Apple ID 右侧的小锁/齿轮图标」这个入口**，
> 应用专用密码也不是在 Sideloadly 里生成的，而是在苹果官网生成、在点 Start 后弹出的密码框里填。

1. 打开 Sideloadly，把刚才的 `HiPDA-nsh-unsigned.ipa` 拖进窗口。
2. **Apple ID** 栏填你的免费 Apple ID 邮箱。
3. **先在浏览器生成应用专用密码**（app-specific password）：
   - 打开 `appleid.apple.com` 登录 → 「登录与安全性」→「应用专用密码」→ 点生成，格式类似 `abcd-efgh-ijkl-mnop`。
   - 这个密码只显示一次，先复制存好。这一步在**苹果官网**做，和 Sideloadly 界面无关。
4. **Bundle Identifier** 保持默认（`cn.hipda.nsh.unofficial`）即可，不用改。
5. **"sign with a paid developer account" 保持不勾选**（我们是免费账号）。
6. 点 **Start**，这时才会弹出登录框：
   - 弹出密码输入框 → 粘贴刚才生成的**应用专用密码**（不是 Apple ID 主密码）。
   - 若接着弹双重认证 → 输入 iPhone/受信任设备上的 6 位验证码。
   - 若它明确要主密码，也可以填主密码 + 验证码，但成功率不如应用专用密码稳。
7. 选 **"Normal" 安装模式** → 等待签名+上传完成。
8. 完成后 iPhone 桌面出现 **HiPDA·nsh 版**，打开即可用。

### 3.3 首次打开

如果提示「无法验证开发者」：
设置 → 通用 → VPN 与设备管理（或「描述文件与设备管理」）→ 点你的 Apple ID → **信任**。

---

## 四、7 天到期后怎么重签

免费证书 7 天到期，到期后 App 会闪退打不开。重签很简单：

1. iPhone 保持数据线连接、信任状态不变。
2. 重新打开 Sideloadly，再拖一次同一个 IPA，点 **Start**。
3. 选 **"Re-sign / retain data"**（保留数据）模式，这样你的登录态、草稿、缓存都还在。
4. 完成后直接打开 App，又能用 7 天。

> 建议设个日历提醒，每 6 天重签一次。也可把 Sideloadly 的 AutoRefresh 之类定时任务配上（进阶，可选）。

---

## 五、App 里的网络权限说明

app 内置了域名白名单，只允许访问：

- `www.4d4y.com`、`4d4y.com`、`*.4d4y.com`

如果帖子里的外链图片不显示，是白名单拦截（详情页会提示缺失域名），不是签名问题。
论坛换域名或新增图片 CDN 时，需要改 `nsh.json` 的 `capabilities.network.hosts` 再重新编译。

---

## 六、排错

| 现象 | 原因 / 处理 |
|---|---|
| Actions 任务红色、日志见 `nsh component add ios` 失败 | runner 选错了。必须是 arm64 的 `macos-15` / `macos-latest`（Apple Silicon M1）；`macos-*-intel` 是 Intel，装不了 iOS 组件。 |
| 日志见版本不匹配 / crate 编译报错 | `NSH_VERSION` 必须和 `Cargo.toml` 的 nsh-* 版本一致（当前 0.5.0）。 |
| 打包时 `ld: Undefined symbols: ___chkstk_darwin` | nsh 0.5.0 的已知 bug：iOS runner 默认按 ios10.0 链接，C 依赖用新 SDK 编译导致不匹配。已在本仓库 workflow 和 `.cargo/config.toml` 里通过 `IPHONEOS_DEPLOYMENT_TARGET=15.0` 修复（见 longbridge/nsh PR #2368）。 |
| **Start 按钮是灰的、点不动** | 不用去 Advanced Options 里勾任何东西（那里是改 Bundle ID/图标/注入插件/Anisette 等高级项）。按顺序查：① 顶部 **Apple ID 栏填邮箱了没**（最常见，留空就亮不了）；若这栏灰掉不能输入，是开了 "Hide Sensitive Information"，去设置里关掉。② iPhone 是否用数据线连上、点了「信任」，窗口顶部应显示设备名/iOS 版本；没识别就拔插数据线、完全退出重开 Sideloadly。③ IPA 是否真加载成功，窗口里应显示应用名「HiPDA·nsh 版」和图标；没有就重新拖一次或点 Select IPA 选文件。 |
| Sideloadly 报 "An error occurred" / 证书相关错误 | 用应用专用密码代替主密码；确认 Apple ID 没开「隐藏邮件地址」；退出 iTunes 重开。 |
| 装完打开闪退 | 没开开发者模式，或没信任描述文件（见 3.1 / 3.3）。 |
| 签名时报 Bundle ID 冲突 | 免费账号同一个 ID 只能签有限个 App；把 Sideloadly 里的 Bundle Identifier 改成一个你自己的唯一串，例如 `cn.hipda.nsh.yourname`。 |
| 想装到 iPad | 可以，NSH 应用同时支持 iPhone 和 iPad，流程完全相同。 |

---

## 七、想省掉每周重签？

两条路，任选：

- **付费开发者账号（$99/年）**：Apple Developer Program。有了它，可以把证书和描述文件配成 GitHub Secrets，让 workflow 直接产出**签名 IPA**，用 AltStore/App Center/TestFlight 分发，一年有效。需要的话我可以帮你把 workflow 改成签名版。
- **巨魔商店（TrollStore）**：永久签名，但对 iOS 版本有要求（依赖特定系统漏洞），iOS 18 大多不在支持范围，**不推荐**。
