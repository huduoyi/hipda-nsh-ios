// `nsh test` 在无界面引擎里打开 Rust 根组件（nsh.json 的 "root"）。
// 这些用例只覆盖纯 UI 状态，不触发网络请求，离线也能跑。
// 注意：默认屏是「论坛首页」，登录页要按首页右上角「登录」才进入。
import { expect, open, test } from "nsh/test";

test("默认进入论坛首页，游客无需登录即可浏览", async () => {
  const app = await open(".");
  expect(app.texts()).toContain("HiPDA·nsh 版");
  expect(app.texts()).toContain("未登录 · 非官方客户端");
  expect(app.texts()).toContain("论坛首页");
  expect(app.texts()).toContain("登录");
  // 游客态要能直接看到版块目录，而不是先撞登录墙。
  expect(app.texts()).toContain("Discovery");
  expect(app.texts().join(" ")).toContain("无需登录");
  expect(app.texts()).toContain("刷新首页");
});

test("登录页说明本机登录态可恢复且可返回首页", async () => {
  const app = await open(".");
  await app.press("登录");
  expect(app.texts()).toContain("登录 HiPDA 论坛");
  expect(app.texts()).toContain("返回首页");
  // 本机保存登录态的说明必须在登录页可见，不能默默持久化。
  expect(app.texts().join(" ")).toContain("下次启动自动恢复");
  expect(app.texts().join(" ")).toContain("密码不会保存");
});

test("空表单提交后才出现字段校验提示", async () => {
  const app = await open(".");
  // 第一次：首页右上角「登录」切到登录页；第二次：登录页「登录」提交空表单。
  await app.press("登录");
  expect(app.texts().join(" ")).not.toContain("⚠ 请输入用户名");
  await app.press("登录");
  expect(app.texts()).toContain("⚠ 请输入用户名");
});

test("论坛首页可直接进入我的空间，游客看到登录引导", async () => {
  const app = await open(".");
  await app.press("我的空间");
  expect(app.texts()).toContain("我的空间");
  expect(app.texts()).toContain("通知");
  expect(app.texts().join(" ")).toContain("游客可搜索与浏览最新帖子");
  expect(app.texts()).toContain("首页");
});

test("首页进入搜索后取消，回到论坛首页而不是版块列表", async () => {
  const app = await open(".");
  await app.press("搜索");
  expect(app.texts()).toContain("搜索论坛主题");
  await app.press("取消");
  expect(app.texts()).toContain("论坛首页");
  expect(app.texts()).toContain("我的空间");
});
