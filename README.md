# hipda-nsh — HiPDA·nsh 版

> **HiPDA 论坛的非官方客户端**。官方 [NSH](https://nsh.dev/zh-CN/docs/) 0.5.0 + Rust，一套代码覆盖 iOS / Android / macOS / Windows / Linux / Web。
>
> 本项目与 HiPDA 论坛（4D4Y）无任何官方关联，仅用于个人学习与使用；请遵守站点用户协议，控制请求频率，不要用于自动化刷帖。

## 工程结构

```
hipda-nsh/
├── nsh.json                    # 应用元数据、root=App、theme、appearance=system、capabilities
├── theme.json                  # 浅色/深色设计令牌（modern-web-guidance 落地）
├── Cargo.toml                  # NSH Rust app + workspace
├── build.rs                    # 把 assets/icon.ico 编进 Windows exe（程序图标）
├── assets/
│   ├── icon.ico                # 程序图标 7 帧：48/64/128/256 用整图，16/24/32 用头部特写
│   ├── icon.png                # 256×256 母版（macOS/Linux 侧备用）
│   └── icon.rc                 # 图标资源脚本：`1 ICON "icon.ico"`（ID 1 是 NSH 窗口类约定）
├── src/
│   ├── main.rs                 # 六端入口（平台 crate 按 target 选择）
│   ├── lib.rs                  # 根组件 App：首页 / 登录 / 版块 / 详情 / 回帖 / 列表 / 我的空间
│   ├── transport.rs            # NSH fetch → forum_core::Transport 适配层
│   └── session.rs              # 会话与回帖草稿的本地持久化（NSH Storage）
├── App.test.tsx                # 无界面组件测试
├── scripts/
│   ├── scan_image_hosts.py     # 外链图片域名白名单巡检
│   ├── make_icon.py            # 从一张方形插画重新生成 assets/icon.ico
│   └── env-win.sh              # Windows(Git Bash) 构建环境：mingw-w64 + cargo 的 PATH
└── crates/forum-core/          # 平台无关业务内核（零 HTTP / 零 UI 依赖）
    ├── src/{config,model,encoding,emoji,transport,parse,auth,post,lists,actions}.rs
    ├── tests/fixtures.rs       # 31 个集成测试
    └── testdata/               # 6 份真实页面样本（游客/登录态首页、GBK 门页、列表/详情）
```



### 分层约定

- `forum-core` 只定义 `Transport` trait，**不 import 任何 NSH / UI / HTTP 库**；`cargo test -p forum-core` 用 `MockTransport` 即可全量验证。
- `src/transport.rs` 是唯一的平台桥：把内核请求翻成 `nsh_core::platform::HttpRequest`，并把 `HttpResponse.body: Vec<u8>` 原样交还内核。**不能用 `HttpResponse::text()`**，它是 UTF-8 lossy，会把 GBK 中文变成 U+FFFD。
- `src/lib.rs` 只做 signal 状态与视图映射，业务判断全部下沉到内核。

## 运行与验证

```sh
# 工具链：nsh 0.5.0 已装到 ~/.nsh，链接 ~/.local/bin/nsh
cd ~/code/hipda-nsh

cargo test --workspace            # Rust 集成测试 + 组件单测
cargo clippy --workspace --all-targets -- -D warnings
~/.local/bin/nsh check            # 类型检查 + cargo check
~/.local/bin/nsh test             # 无界面组件测试（App.test.tsx + cargo test）
~/.local/bin/nsh run              # 本机原生窗口（macOS AppKit）热重载
python3 scripts/scan_image_hosts.py   # 巡检外链图片域名白名单
```

**Rust app 的启动参数限制**（2026-11-13 实测）：`nsh run` 的 `--name`、`--window-size`、`--display` 只对 TSX 入口生效，传了会直接报错。应用名取自 `nsh.json` 的 `name`；`src/main.rs` 的 `AppOptions { size }` 只在直接启动独立二进制时生效，`nsh run` 的开发宿主使用自己的窗口尺寸。`--target web` 需先 `nsh component add web` 下载 Web runtime，本机 GitHub 直连过慢，暂未启用。

也可以不走开发宿主，直接编译独立原生二进制（900×680 窗口，无热重载）：

```sh
cargo run --bin hipda-nsh
```

已验证结果（2026-11-13 晚）：`cargo test --workspace` **68 个 Rust 测试全绿**（forum-core 15 个单测 + 31 个集成测试 + 根组件 22 个单测）；`cargo clippy --workspace --all-targets -- -D warnings` 无告警；`nsh check` 通过；`nsh test` **18/18 通过**（13 个 Rust 单测 + 5 个无界面组件测试）。**macOS 实机冒烟通过**：独立二进制 `cargo run --bin hipda-nsh` 首屏直接渲染真实论坛首页（游客态「技术版区」8 个版块，含描述、主题/帖数、最后发表），GBK 解码 → HTML 解析 → 视图渲染整条链路贯通；`nsh run` 开发宿主窗口亦验证过登录 → 版块列表 → 详情 → 回帖。`nsh test -v` 只剩 macOS 链接器 `__eh_frame section too large` 性能提示，与项目代码无关。新增的个人功能、投票、表情与外链白名单逻辑用 Rust 单测 + 无界面组件测试覆盖；真实私信发送 / 投票提交等写操作仍需登录态实机复核。

> **开发宿主缓存提示**：`nsh run` 会复用上次会话的窗口；若改了首屏逻辑但窗口仍显示旧界面，用 `cargo run --bin hipda-nsh` 起独立二进制复核，或先退出宿主再重启。

### 调试与复核工具

```sh
# 直接把窗口落在某个主题上（截图 / 冒烟用，正常启动不设即无影响）
HIPDA_DEBUG_TID=448060 ~/.local/bin/nsh run

# 解析一份保存下来的主题页，确认「图片/正文没显示」是解析问题还是渲染问题
curl -o /tmp/t.html 'https://www.4d4y.com/forum/viewthread.php?tid=448060'
cargo run -p forum-core --example dump_thread -- /tmp/t.html

# 真实登录态下的写操作复核（默认只读；--yes 才会真的 POST）
cargo run -p forum-core --example verify_writes -- --offline-retry   # GET 重试 3 次 / POST 只发 1 次
cargo run -p forum-core --example verify_writes -- --session <快照.json>
```

### 网络权限

`nsh.json` 显式声明 capabilities（NSH 中「未列出即拒绝」）：

```json
"capabilities": {
  "network": { "hosts": ["www.4d4y.com", "4d4y.com", "img02.4d4y.com", "*.4d4y.com"] },
  "statusBar": true,
  "picker": true
}
```

`statusBar` 是无界面测试运行器所需，`picker` 用于「插入图片」的系统文件选择框。若站点更换域名、新增图片 CDN，或帖子里的**外链图片**指向别的域名，需同步更新 `hosts`，否则图片会被 NSH 拒绝加载。独立应用不写 capabilities 可获无限制权限，但本项目按最小权限声明。

### 外链图片域名白名单维护

`nsh.json` 是**唯一事实来源**：`src/lib.rs` 编译期嵌入该文件，运行时读 `capabilities.network.hosts`；内核 `image_host` / `image_host_allowed` / `unlisted_image_hosts` 负责判定，主题详情只放行已声明域名的图片，未声明的不发起请求，并在详情页显示缺失域名提示（不会静默丢图）。`*.4d4y.com` 覆盖论坛自有子域，页面里的相对地址由 `forum_url()` 拼到主站，也不需要额外授权。

维护流程（发现新外链图片域名时）：

1. 把新帖页面保存为 HTML（GBK / UTF-8 均可），例如 `/tmp/thread.html`。
2. `python3 scripts/scan_image_hosts.py /tmp/thread.html`：脚本默认扫描仓库内 6 份真实样本，再扫追加文件，按 `src` / `file` 属性提取绝对图片域名，输出未声明域名、出现次数、样例与可直接粘贴的 hosts 条目；发现缺口时退出码为 1。加 `--json` 可接入 CI。
3. **人工确认域名可信**后，把域名加入 `nsh.json` 的 `hosts`；不要为了省事直接写 `*`（最小权限原则）。
4. 重跑 `cargo test --workspace` 与 `nsh test`。`sample_pages_have_no_unlisted_image_hosts` 会对全部真实样本做同样的核对，新样本引入未声明域名时测试会失败并列出域名；`image_host_policy_handles_wildcards_and_relative_urls` 锁定大小写、端口、协议相对 URL 与通配符行为。

脚本只读，不自动改 `nsh.json`。论坛自己的表情图是相对路径短码，不走外链白名单。

## 写操作真实登录态复核

内核的写操作（私信、投票、黑名单）原先只有 `MockTransport` 覆盖，站点真实响应没验过。`crates/forum-core/examples/verify_writes.rs` 用 `curl` 实现 `Transport`，直接走 `ForumClient` 的真实代码路径，逐步打印**站点原文**与**该步 POST 次数**（POST 必须恰好 1 次）：

```sh
# 1. 从 App 的登录态取 cookie（NSH 平台 cookie jar，0600 落盘，不打印内容）
python3 scripts/dump_cookies.py            # 写入 ~/.cache/hipda/cookies.txt

# 2. 只读预检：身份 / 资料 / 黑名单名单 / 投票表单
cargo run -p forum-core --example verify_writes -- --cookie-file ~/.cache/hipda/cookies.txt

# 3. 真实写操作（必须显式 --yes）
cargo run -p forum-core --example verify_writes -- --cookie-file ~/.cache/hipda/cookies.txt --yes \
    --blacklist-user <用户名> --pm-target <uid> --pm-text "..." \
    --vote-fid <fid> --vote-tid <tid> --vote-option <选项id>
```


## 正文渲染（块级结构）

Discuz 的正文是 HTML：引用是 `div.quote > blockquote`，列表是 `<ul type="1">`，
代码块是 `div.blockcode > ol > li`。早先解析器把它们全部 `el.text()` 拍平成一段纯文本，
到了界面上就是「一堵墙」——引用不知道谁说的，列表没有符号，代码变成 1./2./3.。

现在 `forum-core` 同时产出两套内容：

| 产物 | 用途 |
|---|---|
| `Post.contents`（扁平 `Content` 流） | 复制正文、搜索摘要 |
| `Post.blocks`（`Block` 块序列） | 界面渲染 |

`Block` 覆盖：段落、引用（含作者与时间）、有序/无序列表、代码块、标题、居中/靠右、
图片、附件、分隔线；引用块可嵌套。渲染在 `render_blocks()`：

- 引用 → 灰底块，有署名时顶部一行「引用 作者 · 时间」，匿名引用不画标题行；
- 列表 → `•` / `1.` 前缀 + 悬挂缩进；
- 代码 → 灰底等宽块，按行保留；
- 标题 → 比正文大一档、半粗；
- 图片 → 复用 `post_image` 的宽高比占位与白名单过滤。

顺手修掉三个只有真实页面才暴露的坑：`<i>` 既当图标又当斜体（原来整段被跳过）、
`div.blockcode` 被当成有序列表、`[backcolor=Yellow]` 这类没解析的 BBCode 直接显示。
回归用例用真实测试帖（tid 3476475「阅读器格式样式样本」）的结构写成。

## 正文里的链接

裸 URL（文字本身就是地址）会被解析成 `Block::Link`，渲染成主色 + 下划线的可点行，
点击交给系统浏览器（`open_url`）。能力声明在 `nsh.json`：

```json
"openUrl": { "schemes": ["https", "http"] }
```

只放行 http/https——`javascript:` 之类的 scheme 一律拒绝，避免正文里的恶意链接
借客户端执行。句子里的锚点（「参见 苹果官网」）仍按普通文字渲染：在句子中间插
一块可点区域会打断阅读，收益不值得。

## 回帖

网页端的规则写在页面 JS 里：`var postminchars = parseInt('5')`，按**字符**计
（中文算 1 个）。客户端在提交前就按同一规则拦一道，少于 5 个字直接提示
「回帖至少 5 个字，当前 N 个字」，不去打一趟注定被驳回的请求。

提交链路有两个容易漏的参数：

| 参数 | 为什么必须有 |
|---|---|
| `replysubmit=yes` | Discuz 的 `post.php` 只有见到它才**处理**提交；不带的 POST 只会把编辑页原样返回（HTTP 200、不跳转），客户端会误判成「发帖结果未知」——表现就是「回帖不能用」 |
| `fid` | 网页端快速回复表单同样带这一项，缺它时部分版块会走到无权限分支 |

排查工具 `crates/forum-core/examples/reply_probe.rs`：用一个**故意过短**的正文
走真实提交，拿到站点的字数拒绝即证明参数正确，且不会留下测试帖。

```sh
cargo run -p forum-core --example reply_probe --offline -- \
    --tid 3476991 --cookie-file ~/.cache/hipda/cookies.txt
```

## 本机缓存（stale-while-revalidate）

浏览速度问题不在带宽，在**等待**：每次进版块、点帖子都要等一个来回，屏幕上就是空白 + 转圈。
`src/cache.rs` 把「先画出来、再偷偷更新」做成默认行为：

| 层 | 作用 | 失效条件 |
|---|---|---|
| 内存（thread-local） | 同一会话内来回翻页零解析、零 IO | 进程退出 |
| `Storage`（版本化键前缀 `hipda.cache.v1.`） | 冷启动也能秒开上次的内容 | 超过 24 小时 / 手动清除 / 条目被淘汰 |

- **命中即渲染**：首页、版块列表、帖子第 1 页都会先把缓存画出来，同时在后台请求最新数据；
  请求失败时**保留缓存**并提示「继续显示本机缓存」，不把已画好的内容换成错误页。
- **预取**：版块列表加载完会自动预取下一页，往下翻时那一屏已经在本地。
- **配额与淘汰**：最多 24 条（约 1–2 MB），超限按写入时间淘汰最旧；单条超过 512 KB
  只留内存不落盘；`Storage` 写满时先淘汰一半再试一次，仍失败就放弃这次缓存 ——
  缓存永远不该影响浏览本身。
- **口径一致**：置顶帖过滤、图片白名单都在同一条 `thread_rows_from` 路径里做，
  首屏 / 翻页 / 预取三处不会漂移。
- **安全**：退出登录会清空缓存（里面可能有登录态才看得到的版块）；设置 → 图片与缓存
  里可以手动清除，并显示当前条数。

命中日志 `[cache] hit detail:448060:1 age=52s` 在 `nsh run` 的输出里可见，
排查「这屏为什么还是慢」时先看它有没有命中。

## 设置

结构对齐原 HiPDA·NG（`~/code/hipda` 的 `SettingActivity`）：顶层是
「客户端发布帖 / 关于 / 检查更新」三条信息 +「分类设置」五个入口，各自进子页。

| 分类 | 本客户端实际生效的项 |
|---|---|
| 论坛选项 | 默认版面（多选，决定列表页顶部标签）、发帖小尾巴（开关 + 文字 + 链接）、黑名单入口 |
| 用户界面 | 正文字号（小/标准/大）、显示头像、圆形头像 |
| 消息通知 | 说明本客户端不常驻后台；入口跳到站内通知列表 |
| 图片与缓存 | 显示正文图片开关；缓存由系统 LRU 管理（无手动清理 API） |
| 其它 | 论坛服务器（只读）、清除本机草稿、版本与声明 |

Android 专属项（手势返回、标题栏动态隐藏、浮动按钮、后台通知任务、屏幕方向）
与没有 API 支撑的项（图片缓存清理、网络缓存）**不列**：点了没反应的开关比没有更糟。
设置落在 `hipda.settings.v1`，字段缺失按默认值补，坏数据不会让 App 起不来。

## 版块右上角菜单

版块列表右上角「⋯」对应原版三点菜单：**发表新帖**（复用编辑器，切到新主题模式，
标题 + 正文，提交走 `post.php?action=newthread` 的 formhash）、**按发表排序**
（`orderby=dateline`）、**显示置顶帖**（关掉后置顶主题不进列表）、**打开站内地址**
（复制 `forumdisplay.php?fid=N` 到剪贴板——没有 `openUrl` 授权，不硬拉浏览器）。

## 界面与可访问性

界面按 `modern-web-guidance` 检索结果落地，要点：

- **首屏是论坛首页**：默认屏恒为 `index.php` 的分区 + 版块索引，游客可直接看到「技术版区」8 个版块及其描述、主题数、最后发表；登录态只改变可见分区（多出「4D4Y」「生活版区」）与可用操作。受限版块由站点返回门页时提示登录，客户端不预先拦截游客浏览。
- **表单**：可见标签用 placeholder + 明确的「登录 HiPDA 论坛」标题；用户名/密码字段设 `AutoFill::Username` / `AutoFill::Password` 并关闭自动纠正与首字母大写，回帖自由文本保留自动纠正；密码框 `masked(true)`。
- **校验时机**：`touched` signal 保证错误只在提交后出现，且一次只报第一个待修正字段；错误文案带「⚠」并说明如何修正，同时 `announce_for_accessibility` 播报，不只用颜色表达。
- **防重复提交**：校验不通过时按钮仍可用；仅在有效提交后才由 `busy` 拦截重复请求。
- **触控与安全区**：所有按钮 / 输入框 `min_h(px(44.0))`，根容器 `p_safe_area()`，列表行 ≥56px。
- **状态不只靠颜色**：置顶 / 新帖用「置顶」「新」文字徽标，页脚状态栏用文字显示「加载中」。
- **深色模式**：`appearance: system` + `theme.json` 的 `light` / `dark` 令牌，文字与背景对比度按 4.5:1 选取；未使用固定高度承载可变文本。
- **错误提示**：论坛跳转提示页里的 `<script>` 文本会被剥离、压平空白并限长，避免整页脚本塞进状态栏。
- **会话与草稿**：登录态写进 NSH `Storage`（原生落文件、Web 落 `localStorage`），重启自动恢复并先向论坛校验 cookie；网络不可达时保留会话不误登出。回帖草稿实时保存、发布后清除。「退出登录」同时清除会话与草稿。
- **楼层头固定**：每层楼顶部是「头像 + 作者 + 时间 + 楼层徽标」，整条用 `sticky().top(0)` 吸顶——滚过长楼层时作者与楼层信息一直可见，不用往上翻。头像由内核按 uid 拼（`Config::avatar_url`，左补零分段），缺 uid 时用作者名首字占位，不留空洞。
- **正文整理**：`tidy_body()` 去掉 `[图片]` 占位（图片已单独渲染）、把连续空行压成一个，避免 Discuz 的 `<br>` 堆出大块空白。
- **正文图片**：详情楼层最多显示 3 张图；图片按**加载到的真实宽高比**占位（`Image::on_load` 回填 `image_ratio`），竖图限宽 62%、横图铺满，`Contain` 不裁剪。相对路径由内核 `forum_url` 拼成绝对地址，非白名单外链域名改为文字提示。
  - 解析层会剔除站点**模板装饰图**（`images/common/none.gif`、`images/default/attachimg.gif`、`images/attachicons/*`、等级星、模板 logo）。`attachimg.gif` 是 1×1 占位符，混进图片列表后被拉到整块高度就是一片空白 —— 这是 2026-11-13「帖子图片不显示」的直接原因。
  - 滚动容器内层**不能**再给 `flex_1()`：内容高度必须由子节点撑开，否则内容被压成视口高度，超出一屏的正文与图片会被裁掉且滚不到。
- **我的空间**：首页、版块列表、通用列表均可进入，9 个入口覆盖通知 / 私信 / 我的帖子 / 我的回复 / 收藏 / 关注 / 最新帖子 / 黑名单 / 搜索；游客态明确提示哪些需要登录，不发注定失败的请求。
- **长列表虚拟化**：主题列表、通知与搜索结果、我的空间、资料、黑名单、表情选择器统一用 `virtual_list`，只渲染视口附近行；表情选择器含 61 个论坛短码，点击插入回帖或私信正文。
- **导航闭环**：搜索页记住来源屏（首页 / 版块列表 / 我的空间），「取消」精确回退；每个子视图都有「首页」，私信详情可「写私信」，资料页可「发私信 / 他的帖子 / 加黑名单」。

## 已实现 / 待实现

已实现：**论坛首页动态解析与游客浏览**（分区 / 版块 / 描述 / 主题数 / 最后发表，站点不可达时回退本地目录）、**登录后固定进入 Discovery 第 1 页**、登录（MD5 + formhash + cookie 会话）、**登录态持久化与退出登录**、**回帖草稿**、版块列表与翻页、主题详情楼层、回帖（含 formhash 重取）、**通用 `ListSource` 列表数据源**（通知 / 搜索 / 私信 / 收藏 / 关注 / 我的帖子 / 我的回复 / 最新帖子 / 用户帖子，含 `searchid` 翻页回放）、**我的空间与用户资料 / 黑名单 / 私信往来与发送**、**投票表单解析与提交**、**61 个表情短码选择器**、**列表虚拟化**、**正文图片显示与本地图片上传**、**外链图片域名白名单机制**、37 个端点、21 个版块、GBK/GB18030 编解码、发帖参数构造、multipart 上传、收藏、CDN 刷新、Session 快照、**61 个论坛表情映射**（`:smile:` / `:)` / `{:2_41:}` 三种短码形式）。

待实现：除 macOS 外的五端实机冒烟（iOS / Android / Windows / Linux / Web，Web 需先解决 runtime 下载）；投票提交按用户决定暂不验证（该账号在所有可见投票帖均无投票权限）。写操作已在真实登录态下复核：黑名单增删、私信发送（含失败分支）通过，详见「写操作真实登录态复核」。

