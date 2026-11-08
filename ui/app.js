import { createDemoBackend } from "./demo.js";

const $ = (id) => document.getElementById(id);
const isDemo = !window.__TAURI__;
const demo = isDemo ? createDemoBackend(window.location.search) : null;
const invoke = (command, args = {}) => {
  if (demo) return demo.invoke(command, args);
  if (typeof window.__TAURI__?.core?.invoke !== "function") {
    return Promise.reject(new Error("Tauri 接口不可用，请检查 withGlobalTauri 配置。未切换到演示模式。"));
  }
  return window.__TAURI__.core.invoke(command, args);
};

const state = {
  snapshot: null,
  selectedGuid: null,
  page: "adapters",
  drafts: new Map(),
  inFlight: false,
  busyLabel: "",
  uncertain: false,
  recoveryLatch: false,
  settingsDirty: false,
  historyPage: 1,
  historyPages: 1,
  lastRead: null,
  pollTimer: null,
  saveContext: null,
  appInfo: null,
  autoRepairAttempts: new Map(),
  autoRepairRunning: false,
  autoRepairTimer: null,
};
const pageTitles = {
  adapters: "网卡管理",
  profiles: "地址方案",
  history: "操作记录",
  settings: "设置",
  about: "关于",
};
const outcomeLabels = {
  success: ["已完成", "success"],
  pending: ["待验证", "warning"],
  failed: ["失败", "danger"],
  rolled_back: ["已回滚", "warning"],
  recovery_required: ["待恢复", "danger"],
};
const actionLabels = {
  apply: "应用地址", apply_mac: "应用地址",
  auto_repair: "自动修复",
  restore: "恢复默认", restore_mac: "恢复默认",
  recover: "恢复配置", recovery: "恢复配置", recover_operation: "恢复配置",
};
const dialogOpeners = new WeakMap();

function element(tag, className = "", text = null) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== null && text !== undefined) node.textContent = String(text);
  return node;
}
function icon(name, className = "icon") {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", className);
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", `#i-${name}`);
  svg.append(use);
  return svg;
}
function text(id, value) {
  const next = value === null || value === undefined || value === "" ? "—" : String(value);
  if ($(id).textContent !== next) $(id).textContent = next;
}
function shown(id, visible) { $(id).hidden = !visible; }
function hasOwn(object, key) { return Object.prototype.hasOwnProperty.call(object, key); }
function expectedMac(adapter) {
  const expectations = state.snapshot?.store.expectations;
  return adapter && expectations && hasOwn(expectations, adapter.guid) ? expectations[adapter.guid] : null;
}
function canonical(value) { return String(value || "").replace(/[:-]/g, "").toUpperCase(); }
function sameMac(a, b) { return canonical(a) === canonical(b); }
function displayMac(value) {
  if (!value) return "—";
  const compact = canonical(value);
  return /^[0-9A-F]{12}$/.test(compact) ? compact.match(/.{2}/g).join("-") : String(value);
}
function validateMac(value) {
  const raw = String(value || "").trim();
  if (!raw) return { valid: false, message: "请输入 MAC 地址。", kind: "" };
  if (!/^(?:[\da-f]{12}|(?:[\da-f]{2}-){5}[\da-f]{2}|(?:[\da-f]{2}:){5}[\da-f]{2})$/i.test(raw)) {
    return { valid: false, message: "需要 12 位十六进制字符。", kind: "invalid" };
  }
  const compact = canonical(raw);
  if (compact === "000000000000" || compact === "FFFFFFFFFFFF") {
    return { valid: false, message: "不能使用全零或广播地址。", kind: "invalid" };
  }
  const first = Number.parseInt(compact.slice(0, 2), 16);
  if (first & 1) return { valid: false, message: "首字节必须为偶数。", kind: "invalid" };
  return { valid: true, mac: compact.match(/.{2}/g).join("-"), message: "格式正确", kind: "valid" };
}
function currentAdapter() {
  return state.snapshot?.adapters.find((adapter) => adapter.guid === state.selectedGuid) || null;
}
function recoveryAdapter() {
  const guid = state.snapshot?.store.pending?.guid;
  return state.snapshot?.adapters.find((adapter) => adapter.guid === guid) || null;
}
function isWifi(adapter) { return /wi-?fi|wireless|无线/i.test(`${adapter.name} ${adapter.description}`); }
function linkStatus(adapter) {
  if (!adapter.admin_enabled) return { label: "已禁用", up: false };
  const value = String(adapter.status || "").toLowerCase();
  if (["up", "connected", "已连接", "1"].includes(value)) return { label: "已连接", up: true };
  const labels = { down: "未连接", disconnected: "未连接", dormant: "休眠", testing: "测试中", unknown: "未知", notpresent: "设备不可用", lowerlayerdown: "底层未连接" };
  return { label: labels[value] || adapter.status || "未知", up: false };
}
function hasDrift(adapter) {
  const expected = expectedMac(adapter);
  return Boolean(expected && !sameMac(expected, adapter.mac));
}
function matchingAdapters() {
  if (!state.snapshot) return [];
  const query = $("adapter-search").value.trim().toLocaleLowerCase();
  return state.snapshot.adapters.filter((adapter) => {
    if (!$("include-virtual").checked && !adapter.hardware) return false;
    return !query || [adapter.name, adapter.description, adapter.guid, adapter.mac, adapter.permanent_mac, adapter.configured_mac]
      .some((value) => String(value || "").toLocaleLowerCase().includes(query));
  });
}
function ensureSelection() {
  const adapters = matchingAdapters();
  if (!adapters.some((adapter) => adapter.guid === state.selectedGuid)) state.selectedGuid = adapters[0]?.guid || null;
  const adapter = currentAdapter();
  if (adapter && !state.drafts.has(adapter.guid)) {
    state.drafts.set(adapter.guid, { value: displayMac(adapter.mac || adapter.configured_mac).replace(/^—$/, ""), dirty: false });
  }
}
function draft() { return state.drafts.get(state.selectedGuid); }
function writeReason(adapter, recovery = false) {
  if (state.inFlight) return "操作进行中。";
  if (!state.snapshot) return "请先读取网卡。";
  if (state.snapshot.store_warning) return "数据文件异常，已禁止写入。";
  if (!state.snapshot.admin) return "需要管理员权限。";
  if (!adapter) return recovery ? "待恢复网卡不可用。" : "请选择网卡。";
  if (!adapter.can_configure) return "此网卡不可配置。";
  if (!recovery && (state.snapshot.store.pending || state.recoveryLatch)) return "请先恢复上次配置。";
  if (!recovery && state.uncertain) return "系统状态未确认，请刷新。";
  return "";
}
function autoRepairReason(adapter) {
  if (!state.snapshot) return "请先读取网卡。";
  if (state.snapshot.store_warning) return "数据文件异常。";
  if (!state.snapshot.store.settings.monitor) return "需开启状态检测。";
  if (!adapter) return "请选择网卡。";
  if (!state.snapshot.admin) return "需要管理员权限。";
  if (state.snapshot.store.pending || state.recoveryLatch) return "需先恢复配置。";
  if (!adapter.can_configure) return "此网卡不可配置。";
  if (!adapter.admin_enabled) return "网卡已禁用。";
  if (!expectedMac(adapter)) return "需先应用地址。";
  return "";
}
function hasAutoRepair() {
  return Object.values(state.snapshot?.store.settings.auto_repair || {}).some(Boolean);
}
function errorText(error) {
  if (typeof error === "string") return error;
  if (error?.message) return String(error.message);
  try { return JSON.stringify(error) || "未知错误"; } catch { return "未知错误"; }
}
function toast(message, kind = "success") {
  const node = element("div", `toast ${kind}`);
  node.append(icon("info"), element("span", "", message));
  const close = element("button", "icon-button");
  close.type = "button";
  close.setAttribute("aria-label", "关闭此提示");
  close.append(icon("close"));
  close.addEventListener("click", () => node.remove());
  node.append(close);
  const region = $("toast-region");
  while (region.childElementCount >= 4) region.firstElementChild.remove();
  region.append(node);
  window.setTimeout(() => node.remove(), kind === "error" ? 10000 : 6500);
}
function showError(error, context = "") {
  const message = `${context ? `${context}：` : ""}${errorText(error)}`;
  text("error-message", message);
  shown("error-banner", true);
  toast(message, "error");
}
function operationResult(result) {
  const [label, kind] = outcomeLabels[result.outcome] || ["未知结果", "danger"];
  $("operation-result").className = `banner ${kind}`;
  text("result-title", label);
  text("result-message", result.message || "后端未返回说明，请检查操作记录与当前地址。");
  shown("operation-result", true);
  toast(`${label}：${result.message || "请查看操作记录。"}`, kind === "danger" ? "error" : kind);
}

// All backend calls (including file import, clipboard work, and polling) share this gate.
async function execute(label, work, { quiet = false } = {}) {
  if (state.inFlight) return { ok: false };
  const returnFocus = document.activeElement;
  state.inFlight = true;
  state.busyLabel = label;
  window.clearTimeout(state.pollTimer);
  renderControls();
  try {
    const value = await work();
    if (!quiet) text("busy-status", `${label}完成`);
    return { ok: true, value };
  } catch (error) {
    showError(error, label);
    return { ok: false, error };
  } finally {
    state.inFlight = false;
    state.busyLabel = "";
    renderControls();
    // Disabling a focused action blurs it. Restore only if the user has not moved on.
    if (document.activeElement === document.body && returnFocus !== document.body && returnFocus?.isConnected && returnFocus.getClientRects().length) {
      if (!returnFocus.disabled) returnFocus.focus({ preventScroll: true });
      else if (!document.querySelector("dialog[open]")) $("page-title").focus({ preventScroll: true });
    }
    schedulePoll();
  }
}
function validateStore(store) {
  if (!store || !Array.isArray(store.profiles) || !Array.isArray(store.history) || !store.settings || !store.expectations || typeof store.expectations !== "object") {
    throw new Error("后端返回的本地数据格式不完整。请检查接口或本地数据文件。");
  }
  return store;
}
function acceptStore(store) {
  validateStore(store);
  if (!state.snapshot) throw new Error("尚未读取快照，无法更新本地数据。");
  state.snapshot.store = store;
  renderAll();
}
async function fetchAppInfo() {
  try {
    state.appInfo = await invoke("app_info");
  } catch {
    state.appInfo = null;
  }
  renderAbout();
  renderControls();
}
async function fetchSnapshot() {
  const snapshot = await invoke("get_snapshot");
  if (!snapshot || !Array.isArray(snapshot.adapters) || typeof snapshot.admin !== "boolean") {
    throw new Error("后端返回的设备快照无效。");
  }
  validateStore(snapshot.store);
  state.snapshot = snapshot;
  state.lastRead = new Date();
  state.uncertain = false;
  state.recoveryLatch = Boolean(snapshot.store.pending);
  ensureSelection();
  renderAll();
}
async function refresh(manual = false) {
  const result = await execute(manual ? "刷新" : "读取快照", async () => {
    await fetchSnapshot();
    if (manual) toast("已刷新。", "info");
  }, { quiet: !manual });
  if (result.ok) scheduleAutoRepair();
  return result;
}
function scheduleAutoRepair() {
  window.clearTimeout(state.autoRepairTimer);
  if (!hasAutoRepair()) return;
  state.autoRepairTimer = window.setTimeout(() => { void runAutoRepair(); }, 400);
}
async function stopAutoRepair(guid, reason) {
  try {
    acceptStore(await invoke("set_auto_repair", { guid, enabled: false }));
  } catch {
    // The write failure itself is shown by the caller.
  }
  toast(`自动修复已停用：${reason}`, "warning");
}
async function runAutoRepair() {
  if (state.inFlight || state.autoRepairRunning || !state.snapshot || document.querySelector("dialog[open]")) return;
  const snapshot = state.snapshot;
  const store = snapshot.store;
  if (!snapshot.admin || snapshot.store_warning || store.pending || !store.settings.monitor) return;
  const now = Date.now();
  const adapter = snapshot.adapters.find((item) => {
    if (!store.settings.auto_repair?.[item.guid]) return false;
    const expected = expectedMac(item);
    if (!expected || sameMac(expected, item.mac)) return false;
    if (!item.can_configure || !item.admin_enabled) return false;
    return now - (state.autoRepairAttempts.get(item.guid) || 0) >= 5 * 60 * 1000;
  });
  if (!adapter) return;
  const expected = expectedMac(adapter);
  if (!expected) return;
  state.autoRepairAttempts.set(adapter.guid, now);
  state.autoRepairRunning = true;
  try {
    await execute("自动修复", async () => {
      let result;
      try {
        result = await invoke("auto_repair_mac", { guid: adapter.guid, mac: expected });
      } catch (error) {
        await stopAutoRepair(adapter.guid, errorText(error));
        throw error;
      }
      try {
        await fetchSnapshot();
      } catch (error) {
        state.uncertain = true;
        throw error;
      }
      if (result.outcome === "success" || result.outcome === "pending") {
        const message = result.outcome === "success" ? `已自动修复 ${adapter.name || "网卡"}。` : `已写入 ${adapter.name || "网卡"}，待验证。`;
        operationResult({ ...result, message });
      } else {
        await stopAutoRepair(adapter.guid, result.message || result.outcome);
        operationResult(result);
      }
    });
  } finally {
    state.autoRepairRunning = false;
    schedulePoll();
  }
}
function schedulePoll() {
  window.clearTimeout(state.pollTimer);
  const settings = state.snapshot?.store.settings || { monitor: true, interval_seconds: 60 };
  const autoActive = hasAutoRepair();
  if (settings.monitor === false || (!autoActive && document.visibilityState === "hidden")) return;
  const interval = [30, 60, 120].includes(settings.interval_seconds) ? settings.interval_seconds : 60;
  state.pollTimer = window.setTimeout(async () => {
    if (!state.inFlight && !document.querySelector("dialog[open]") && (document.visibilityState !== "hidden" || autoActive)) await refresh(false);
    else schedulePoll();
  }, interval * 1000);
}
function replacePreservingFocus(container, content) {
  const active = document.activeElement;
  const key = container.contains(active) ? active.dataset.focusKey : null;
  container.replaceChildren(content);
  if (key) {
    const replacement = Array.from(container.querySelectorAll("[data-focus-key]")).find((node) => node.dataset.focusKey === key);
    if (replacement && !replacement.disabled) replacement.focus({ preventScroll: true });
  }
}
function emptyState(title, description, iconName) {
  const box = element("div", "empty-state");
  box.append(icon(iconName, "empty-icon"), element("strong", "", title), element("p", "", description));
  return box;
}
function renderAdapters() {
  if (!state.snapshot) return;
  ensureSelection();
  const adapters = matchingAdapters();
  const totalPhysical = state.snapshot.adapters.filter((adapter) => adapter.hardware).length;
  text("adapter-count", `${adapters.length} / ${totalPhysical} 物理`);
  const fragment = document.createDocumentFragment();
  for (const adapter of adapters) {
    const status = linkStatus(adapter);
    const button = element("button", `adapter-card${adapter.guid === state.selectedGuid ? " selected" : ""}`);
    button.type = "button";
    button.dataset.guid = adapter.guid;
    button.dataset.focusKey = adapter.guid;
    button.setAttribute("aria-pressed", String(adapter.guid === state.selectedGuid));
    button.setAttribute("aria-label", `${adapter.name || "未命名网卡"}，${status.label}，${displayMac(adapter.mac)}`);
    button.title = adapter.description || adapter.name || "网卡";
    const content = element("span", "adapter-card-content");
    content.append(element("span", "adapter-card-title", adapter.name || "未命名网卡"));
    content.append(element("span", "adapter-card-description", adapter.description || "—"));
    content.append(element("span", "adapter-card-mac mac", displayMac(adapter.mac)));
    const statusLine = element("span", "adapter-card-status");
    statusLine.append(element("span", `status-dot${status.up ? " up" : ""}`), element("span", "", status.label));
    if (hasDrift(adapter)) statusLine.append(element("span", "drift-dot", adapter.mac ? "地址偏移" : "待验证"));
    content.append(statusLine);
    button.append(icon(isWifi(adapter) ? "wifi" : "network"), content);
    fragment.append(button);
  }
  if (!adapters.length) {
    fragment.append(emptyState(state.snapshot.adapters.length ? "没有匹配的网卡" : "暂无网卡", state.snapshot.adapters.length ? "清空搜索或显示虚拟网卡。" : "检查设备后刷新。", "network"));
  }
  replacePreservingFocus($("adapter-list"), fragment);
  renderDetails();
}
function renderDetails() {
  const adapter = currentAdapter();
  shown("no-selection", !adapter);
  shown("adapter-detail", Boolean(adapter));
  if (!adapter) { renderControls(); return; }
  const status = linkStatus(adapter);
  text("adapter-name", adapter.name);
  text("adapter-description", adapter.description);
  $("detail-icon").querySelector("use").setAttribute("href", isWifi(adapter) ? "#i-wifi" : "#i-network");
  text("adapter-link", status.label);
  $("adapter-link").className = `badge ${status.up ? "success" : "neutral"}`;
  text("adapter-type", adapter.hardware ? "物理网卡" : "虚拟网卡");
  text("adapter-capability", adapter.can_configure ? "可配置" : "不可配置");
  text("current-mac", displayMac(adapter.mac));
  text("permanent-mac", displayMac(adapter.permanent_mac));
  text("configured-mac", displayMac(adapter.configured_mac));
  text("expected-mac", displayMac(expectedMac(adapter)));
  text("adapter-guid", adapter.guid);
  text("adapter-pnp", adapter.pnp_id);
  shown("config-error", Boolean(adapter.config_error));
  text("config-error", adapter.config_error);
  shown("drift-banner", hasDrift(adapter));
  if (hasDrift(adapter)) {
    text("drift-title", adapter.mac ? "地址与目标不一致" : "待验证地址");
    text("drift-message", `目标 ${displayMac(expectedMac(adapter))}。`);
  }
  // Drafts belong to an adapter, not a snapshot. Polling must never replace typed text.
  const value = draft()?.value || "";
  if ($("mac-input").value !== value) $("mac-input").value = value;
  renderProfileChooser();
  renderValidation();
  renderControls();
}
function renderValidation() {
  const validation = validateMac(draft()?.value || "");
  text("mac-validation", validation.message);
  $("mac-validation").className = `validation ${validation.kind}`;
  $("mac-input").setAttribute("aria-invalid", String(validation.kind === "invalid"));
}
function renderProfileChooser() {
  const select = $("profile-select");
  const previous = select.value;
  const fragment = document.createDocumentFragment();
  const placeholder = element("option", "", "选择地址方案");
  placeholder.value = "";
  fragment.append(placeholder);
  for (const profile of state.snapshot?.store.profiles || []) {
    const option = element("option", "", `${profile.name} · ${displayMac(profile.mac)}${profile.adapter_guid ? " · 已绑定" : ""}`);
    option.value = profile.id;
    fragment.append(option);
  }
  select.replaceChildren(fragment);
  if (Array.from(select.options).some((option) => option.value === previous)) select.value = previous;
}
function profileAdapterName(profile) {
  if (!profile.adapter_guid) return "不限网卡";
  return state.snapshot?.adapters.find((adapter) => adapter.guid === profile.adapter_guid)?.name || `设备未连接 · ${profile.adapter_guid}`;
}
function actionButton(action, id, label, iconName, extraClass = "") {
  const button = element("button", `button small ${extraClass}`);
  button.type = "button";
  button.dataset.action = action;
  button.dataset.id = id;
  button.dataset.focusKey = `${action}:${id}`;
  button.append(icon(iconName), element("span", "", label));
  button.setAttribute("aria-label", `${label}方案`);
  button.disabled = state.inFlight;
  return button;
}
function renderProfiles() {
  if (!state.snapshot) return;
  const query = $("profile-search").value.trim().toLocaleLowerCase();
  const all = state.snapshot.store.profiles;
  const profiles = all.filter((profile) => [profile.name, profile.mac, profile.adapter_guid, profileAdapterName(profile)].some((value) => String(value || "").toLocaleLowerCase().includes(query)));
  text("profile-count", all.length);
  text("profiles-summary", query ? `${profiles.length} / ${all.length} 个方案` : `${all.length} 个方案`);
  const fragment = document.createDocumentFragment();
  for (const profile of profiles) {
    const card = element("article", "profile-card");
    const header = element("div", "profile-card-header");
    header.append(icon("profile"), element("h3", "", profile.name));
    const meta = element("div", "profile-meta");
    meta.append(element("span", "", profileAdapterName(profile)), element("span", "", `创建于 ${formatTime(profile.created_at)}`));
    const actions = element("div", "profile-card-actions");
    actions.append(actionButton("copy", profile.id, "复制", "copy"), actionButton("load", profile.id, "载入", "arrow"), actionButton("delete", profile.id, "删除", "trash", "danger-outline"));
    for (const button of actions.children) button.setAttribute("aria-label", `${button.textContent}方案「${profile.name}」`);
    card.append(header, element("span", "mac", displayMac(profile.mac)), meta, actions);
    fragment.append(card);
  }
  if (!profiles.length) fragment.append(emptyState(query ? "没有匹配的方案" : "暂无方案", query ? "更换搜索条件。" : "从网卡管理保存，或导入 JSON。", "profile"));
  replacePreservingFocus($("profile-list"), fragment);
}
function formatTime(value) {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return String(value);
  return new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false }).format(date);
}
function renderHistory() {
  const query = $("history-search").value.trim().toLocaleLowerCase();
  const history = (state.snapshot?.store.history || []).filter((entry) => {
    const values = [entry.adapter_name, entry.adapter_guid, entry.action, actionLabels[entry.action], entry.requested_mac, entry.previous_mac, entry.actual_mac, entry.outcome, outcomeLabels[entry.outcome]?.[0], entry.message, entry.time, formatTime(entry.time)];
    return values.some((value) => String(value || "").toLocaleLowerCase().includes(query));
  });
  const size = Number($("history-size").value);
  state.historyPages = Math.max(1, Math.ceil(history.length / size));
  state.historyPage = Math.max(1, Math.min(state.historyPage, state.historyPages));
  const entries = history.slice((state.historyPage - 1) * size, state.historyPage * size);
  text("history-summary", `${history.length} 条记录`);
  const fragment = document.createDocumentFragment();
  for (const entry of entries) {
    const row = element("tr");
    const deviceCell = element("td");
    deviceCell.append(element("span", "", formatTime(entry.time)), element("span", "subline", entry.adapter_name || entry.adapter_guid || "—"));
    const actionCell = element("td", "", actionLabels[entry.action] || entry.action || "—");
    const previousCell = element("td");
    previousCell.append(element("span", "mac", displayMac(entry.previous_mac)));
    const addressCell = element("td");
    addressCell.append(element("span", "mac", displayMac(entry.requested_mac)), element("span", "subline mac", `实际 ${displayMac(entry.actual_mac)}`));
    const resultCell = element("td");
    const [label, kind] = outcomeLabels[entry.outcome] || [entry.outcome || "未知", "neutral"];
    resultCell.append(element("span", `badge ${kind}`, label), element("span", "history-message", entry.message || "—"));
    row.append(deviceCell, actionCell, previousCell, addressCell, resultCell);
    fragment.append(row);
  }
  $("history-body").replaceChildren(fragment);
  shown("history-empty", !entries.length);
  $("history-empty").querySelector("strong").textContent = query ? "没有匹配的记录" : "暂无操作记录";
  $("history-empty").querySelector("p").textContent = query ? "尝试其他网卡、地址、状态或消息。" : "网卡写入尝试会记录在这里。";
  text("history-page", `${state.historyPage} / ${state.historyPages}`);
  $("history-prev").disabled = state.historyPage <= 1;
  $("history-next").disabled = state.historyPage >= state.historyPages;
}
function renderSettings() {
  const settings = state.snapshot?.store.settings;
  if (settings && !state.settingsDirty) {
    $("monitor-input").checked = settings.monitor !== false;
    $("interval-input").value = String([30, 60, 120].includes(settings.interval_seconds) ? settings.interval_seconds : 60);
  }
  text("settings-state", state.settingsDirty ? "未保存" : settings ? "已保存" : "程序运行时生效");
  text("data-directory", state.snapshot?.data_dir);
}
function renderAbout() {
  const version = state.appInfo?.version;
  text("about-version", version ? `v${version}` : "—");
  text("about-data-dir", state.snapshot?.data_dir);
  $("open-repo").disabled = !state.appInfo;
  $("open-releases").disabled = !state.appInfo;
}
function renderGlobal() {
  const snapshot = state.snapshot;
  text("admin-label", snapshot ? snapshot.admin ? "管理员权限" : "只读模式" : "检查权限");
  $("admin-pill").classList.toggle("is-admin", Boolean(snapshot?.admin));
  $("admin-pill").classList.toggle("is-readonly", Boolean(snapshot) && !snapshot.admin);
  shown("store-warning", Boolean(snapshot?.store_warning));
  text("store-warning-text", snapshot?.store_warning);
  const pending = snapshot?.store.pending;
  shown("recovery-banner", Boolean(pending || state.recoveryLatch));
  if (pending) {
    const unavailable = !recoveryAdapter() ? "设备未连接。" : !recoveryAdapter().can_configure ? "设备不可配置。" : !snapshot.admin ? "需要管理员权限。" : "";
    text("recovery-message", `${pending.adapter_name || pending.guid}：上次修改未完成。${unavailable}`);
  } else if (state.recoveryLatch) {
    text("recovery-message", "恢复状态未知，请刷新确认。");
  }
  shown("uncertain-banner", state.uncertain);
  if (state.lastRead) {
    const clock = new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false }).format(state.lastRead);
    const settings = snapshot.store.settings;
    text("refresh-status", `${clock} · ${settings.monitor !== false ? `${settings.interval_seconds || 60} 秒检测` : "手动检测"}`);
  }
}
function renderControls() {
  const busy = state.inFlight;
  const ready = Boolean(state.snapshot);
  const writable = ready && !state.snapshot.store_warning;
  const adapter = currentAdapter();
  const validation = validateMac(draft()?.value || "");
  const reason = writeReason(adapter);
  $("refresh-button").disabled = busy;
  $("elevate-button").hidden = !ready || Boolean(state.snapshot?.admin);
  $("elevate-button").disabled = busy;
  $("elevate-button").title = "提升权限";
  const editingLocked = busy && !["读取快照", "刷新列表"].includes(state.busyLabel);
  $("mac-input").disabled = editingLocked || !adapter;
  $("random-button").disabled = busy || !adapter;
  $("apply-button").disabled = Boolean(reason) || !validation.valid;
  $("apply-button").title = reason || (!validation.valid ? validation.message : "写入后校验实际地址");
  $("restore-button").disabled = Boolean(reason);
  $("restore-button").title = reason || "移除注册表覆盖";
  const autoReason = autoRepairReason(adapter);
  $("auto-repair-input").checked = Boolean(adapter && state.snapshot?.store.settings.auto_repair?.[adapter.guid]);
  $("auto-repair-input").disabled = busy || Boolean(autoReason) || !adapter;
  $("auto-repair-input").title = autoReason || "检测到漂移时自动重应用";
  text("auto-repair-note", autoReason || "程序运行时生效");
  $("save-profile-button").disabled = busy || !writable || !adapter || !validation.valid;
  $("profile-select").disabled = busy || !ready || !state.snapshot.store.profiles.length;
  $("load-profile-button").disabled = busy || !$("profile-select").value;
  $("load-expectation").disabled = busy || !expectedMac(adapter);
  $("recover-button").disabled = !state.snapshot?.store.pending || Boolean(writeReason(recoveryAdapter(), true));
  $("recover-button").title = writeReason(recoveryAdapter(), true) || "恢复日志记录的上次配置";
  for (const id of ["import-button", "export-button"]) $(id).disabled = busy || !writable;
  for (const id of ["open-data-button", "open-wifi-button"]) $(id).disabled = busy || !ready;
  for (const id of ["open-repo", "open-releases"]) $(id).disabled = busy || !state.appInfo;
  $("monitor-input").disabled = busy || !writable;
  $("interval-input").disabled = busy || !writable || !$("monitor-input").checked;
  $("save-settings-button").disabled = busy || !writable || !state.settingsDirty;
  $("demo-scenario").disabled = busy;
  for (const id of ["profile-name", "bind-profile", "save-confirm", "save-cancel"]) $(id).disabled = busy;
  for (const button of $("profile-list").querySelectorAll("button[data-action]")) button.disabled = busy || (!writable && button.dataset.action === "delete");
  text("write-reason", reason || (adapter && !adapter.admin_enabled ? "网卡已禁用，写入后保持禁用。" : "应用时可能短暂断网。"));
  $("main-content").setAttribute("aria-busy", String(busy));
  text("busy-status", busy ? "处理中…" : isDemo ? "演示" : "就绪");
}
function renderAll() {
  renderGlobal();
  renderAdapters();
  renderProfiles();
  renderHistory();
  renderSettings();
  renderAbout();
  renderControls();
}
function switchPage(page, focus = false) {
  if (!hasOwn(pageTitles, page)) return;
  state.page = page;
  for (const key of Object.keys(pageTitles)) shown(`page-${key}`, key === page);
  for (const button of document.querySelectorAll(".nav-item")) {
    const active = button.dataset.page === page;
    button.classList.toggle("active", active);
    if (active) button.setAttribute("aria-current", "page");
    else button.removeAttribute("aria-current");
  }
  text("page-title", pageTitles[page]);
  $("main-content").scrollTop = 0;
  if (focus) $("page-title").focus({ preventScroll: true });
}
function openDialog(dialog, focusTarget) {
  if (document.querySelector("dialog[open]")) return false;
  dialogOpeners.set(dialog, document.activeElement);
  dialog.showModal();
  if (focusTarget) focusTarget.focus();
  return true;
}
for (const dialog of document.querySelectorAll("dialog")) {
  dialog.addEventListener("close", () => {
    const opener = dialogOpeners.get(dialog);
    if (opener?.isConnected && !opener.disabled && opener.getClientRects().length) opener.focus({ preventScroll: true });
    else $("page-title").focus({ preventScroll: true });
    schedulePoll();
    scheduleAutoRepair();
  });
}
function confirmAction({ title, description, details = [], warnings = [], accept = "确认", destructive = true }) {
  if (document.querySelector("dialog[open]")) return Promise.resolve(false);
  text("confirm-title", title);
  text("confirm-description", description);
  const detailFragment = document.createDocumentFragment();
  for (const [label, value] of details) detailFragment.append(element("dt", "", label), element("dd", /地址/.test(label) ? "mac" : "", value || "—"));
  $("confirm-details").replaceChildren(detailFragment);
  shown("confirm-details", details.length > 0);
  $("confirm-warnings").replaceChildren(...warnings.map((warning) => element("li", "", warning)));
  shown("confirm-warnings", warnings.length > 0);
  text("confirm-accept", accept);
  $("confirm-accept").className = `button ${destructive ? "danger-outline" : "primary"}`;
  $("confirm-symbol").className = `modal-symbol${destructive ? "" : " teal"}`;
  const dialog = $("confirm-dialog");
  dialog.returnValue = "cancel";
  return new Promise((resolve) => {
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "confirm"), { once: true });
    openDialog(dialog, $("confirm-cancel"));
  });
}
const nicWarnings = [
  "可能短暂断网，请勿在远程会话中操作当前网卡。",
  "原禁用网卡保持禁用，不会为了写入而启用。",
  "驱动可能不支持该地址，写入不保证生效。",
];
async function writeNic(kind) {
  const recovery = kind === "recover";
  const adapter = recovery ? recoveryAdapter() : currentAdapter();
  const reason = writeReason(adapter, recovery);
  if (reason) { showError(reason); return; }
  if (recovery && !state.snapshot.store.pending) { showError("没有可恢复的操作日志。"); return; }
  const validation = validateMac(draft()?.value || "");
  if (kind === "apply" && !validation.valid) { showError(validation.message); return; }
  const title = kind === "apply" ? "应用这个地址？" : kind === "restore" ? "恢复默认地址？" : "恢复上次配置？";
  const description = kind === "apply" ? "将写入并重启所选网卡，网络可能短暂中断。" : kind === "restore" ? "将移除注册表覆盖，最终地址由驱动决定。" : "将还原操作前的覆盖配置和启用状态。";
  const details = [["网卡", adapter.name || adapter.guid], ["当前地址", displayMac(adapter.mac)], ["目标地址", kind === "apply" ? validation.mac : kind === "restore" ? "默认（由驱动决定）" : displayMac(state.snapshot.store.pending.previous_mac)], ["设备状态", adapter.admin_enabled ? "已启用" : "已禁用，保持禁用"]];
  if (recovery) details.push(["操作前状态", state.snapshot.store.pending.was_enabled ? "已启用" : "已禁用，保持禁用"]);
  const warnings = recovery ? [nicWarnings[0], "将还原操作前的启用或禁用状态。", nicWarnings[2]] : nicWarnings;
  const confirmed = await confirmAction({ title, description, details, warnings, accept: kind === "apply" ? "应用" : kind === "restore" ? "恢复默认" : "恢复" });
  if (!confirmed) return;
  const latestAdapter = state.snapshot?.adapters.find((item) => item.guid === adapter.guid);
  const freshReason = writeReason(latestAdapter, recovery);
  if (freshReason) { showError(freshReason); return; }
  const command = kind === "apply" ? "apply_mac" : kind === "restore" ? "restore_mac" : "recover_operation";
  const args = kind === "apply" ? { guid: adapter.guid, mac: validation.mac } : kind === "restore" ? { guid: adapter.guid } : {};
  await execute(kind === "apply" ? "应用地址" : kind === "restore" ? "恢复默认" : "恢复上次配置", async () => {
    state.uncertain = true;
    let result;
    let commandError;
    let snapshotError;
    try {
      result = await invoke(command, args);
      if (!result || !hasOwn(outcomeLabels, result.outcome)) throw new Error("写入返回了未知结果，必须刷新确认系统状态。");
    } catch (error) {
      commandError = error;
    }
    // A rejected write may still have touched the device. Always attempt a new snapshot.
    try { await fetchSnapshot(); } catch (error) { snapshotError = error; }
    if (result?.outcome === "recovery_required") state.recoveryLatch = true;
    if (snapshotError) state.uncertain = true;
    renderGlobal();
    if (result && hasOwn(outcomeLabels, result.outcome)) operationResult(result);
    if (commandError || snapshotError) {
      const messages = [];
      if (commandError) messages.push(errorText(commandError));
      if (snapshotError) messages.push(`操作后刷新失败：${errorText(snapshotError)}。已暂停新的网卡写入。`);
      throw new Error(messages.join("\n"));
    }
  });
}
async function setTarget(guid, mac, source) {
  if (state.inFlight) return;
  const validation = validateMac(mac);
  if (!validation.valid) { showError(`${source}中的地址无效：${validation.message}`); return; }
  const adapter = state.snapshot?.adapters.find((item) => item.guid === guid);
  if (!adapter) { showError("网卡不可用，请刷新。"); return; }
  const previous = state.drafts.get(guid);
  if (previous?.dirty && previous.value && !sameMac(previous.value.trim(), validation.mac)) {
    if (!await confirmAction({ title: "替换草稿？", description: "载入只更新编辑框，不写入网卡。", details: [["网卡", adapter.name], ["现有草稿", previous.value], ["载入地址", validation.mac]], accept: "替换", destructive: false })) return;
  }
  $("adapter-search").value = "";
  if (!adapter.hardware) $("include-virtual").checked = true;
  state.selectedGuid = guid;
  state.drafts.set(guid, { value: validation.mac, dirty: true });
  switchPage("adapters");
  renderAdapters();
  $("mac-input").focus();
  toast(`已载入${source}。`, "info");
}
async function loadProfile(id) {
  const profile = state.snapshot?.store.profiles.find((item) => item.id === id);
  if (!profile) { showError("方案不存在。"); return; }
  const guid = profile.adapter_guid || state.selectedGuid;
  if (!guid) { showError("请先选择网卡。"); return; }
  await setTarget(guid, profile.mac, `方案「${profile.name}」`);
}
function openSaveDialog() {
  const adapter = currentAdapter();
  const validation = validateMac(draft()?.value || "");
  if (state.inFlight || !adapter || !validation.valid) return;
  state.saveContext = { guid: adapter.guid, mac: validation.mac };
  $("profile-name").value = "";
  $("bind-profile").checked = false;
  text("save-mac", validation.mac);
  text("bind-description", `当前网卡：${adapter.name || adapter.guid}`);
  shown("profile-form-error", false);
  openDialog($("save-dialog"), $("profile-name"));
}
async function copyProfile(profile) {
  const result = await execute("复制地址", async () => {
    if (!navigator.clipboard?.writeText) throw new Error("此环境不支持剪贴板访问。");
    await navigator.clipboard.writeText(displayMac(profile.mac));
    toast("地址已复制。");
  });
  if (!result.ok && result.error) {
    $("copy-value").value = displayMac(profile.mac);
    if (openDialog($("copy-dialog"), $("copy-value"))) $("copy-value").select();
  }
}
async function deleteProfile(profile) {
  if (state.inFlight) return;
  const confirmed = await confirmAction({ title: "删除方案？", description: "不会修改网卡。此操作无法撤销。", details: [["方案名称", profile.name], ["方案地址", displayMac(profile.mac)]], accept: "删除" });
  if (!confirmed) return;
  const result = await execute("删除方案", async () => {
    acceptStore(await invoke("delete_profile", { id: profile.id }));
    toast("方案已删除。");
  });
  if (result.ok && state.page === "profiles") {
    const next = $("profile-list").querySelector('[data-action="delete"]') || $("profile-search");
    next.focus({ preventScroll: true });
  }
}

// Navigation and editing remain local; no implicit write is triggered by a selection.
for (const button of document.querySelectorAll(".nav-item")) button.addEventListener("click", () => switchPage(button.dataset.page, true));
$("refresh-button").addEventListener("click", () => refresh(true));
$("dismiss-error").addEventListener("click", () => shown("error-banner", false));
$("dismiss-result").addEventListener("click", () => shown("operation-result", false));
$("adapter-search").addEventListener("input", renderAdapters);
$("include-virtual").addEventListener("change", renderAdapters);
$("adapter-list").addEventListener("click", (event) => {
  const button = event.target.closest("button[data-guid]");
  if (!button) return;
  state.selectedGuid = button.dataset.guid;
  ensureSelection();
  renderAdapters();
});
$("mac-input").addEventListener("input", () => {
  if (!state.selectedGuid) return;
  state.drafts.set(state.selectedGuid, { value: $("mac-input").value, dirty: true });
  renderValidation();
  renderControls();
});
$("mac-input").addEventListener("blur", () => {
  const validation = validateMac(draft()?.value || "");
  if (validation.valid) {
    draft().value = validation.mac;
    $("mac-input").value = validation.mac;
  }
});
$("random-button").addEventListener("click", async () => {
  const guid = state.selectedGuid;
  if (!guid) return;
  await execute("生成随机地址", async () => {
    const validation = validateMac(await invoke("random_mac"));
    if (!validation.valid) throw new Error("后端生成的地址无效。");
    state.drafts.set(guid, { value: validation.mac, dirty: true });
    if (state.selectedGuid === guid) renderDetails();
  });
  if (state.selectedGuid === guid) $("mac-input").focus();
});
$("apply-button").addEventListener("click", () => writeNic("apply"));
$("restore-button").addEventListener("click", () => writeNic("restore"));
$("recover-button").addEventListener("click", () => writeNic("recover"));
$("load-expectation").addEventListener("click", () => setTarget(state.selectedGuid, expectedMac(currentAdapter()), "上次目标"));
$("auto-repair-input").addEventListener("change", async (event) => {
  if (state.inFlight) return;
  const adapter = currentAdapter();
  if (!adapter) return;
  const enabled = event.target.checked;
  if (enabled) {
    const confirmed = await confirmAction({
      title: "启用自动修复？",
      description: "检测到地址漂移时会自动重应用，可能短暂断网。仅在本程序运行期间生效。",
      warnings: ["状态检测必须保持开启。", "连续失败或恢复未完成时会自动停用。"],
      accept: "启用",
      destructive: false,
    });
    if (!confirmed) {
      event.target.checked = false;
      return;
    }
  }
  const result = await execute(enabled ? "启用自动修复" : "停用自动修复", async () => {
    acceptStore(await invoke("set_auto_repair", { guid: adapter.guid, enabled }));
    toast(enabled ? "自动修复已启用。" : "自动修复已停用。", "info");
  });
  if (!result.ok) event.target.checked = !enabled;
  renderControls();
  scheduleAutoRepair();
});
for (const [id, page] of [["open-repo", "repo"], ["open-releases", "releases"]]) {
  $(id).addEventListener("click", () => execute("打开链接", async () => {
    await invoke("open_project_page", { page });
    if (isDemo) toast("演示环境。", "info");
  }));
}
$("profile-select").addEventListener("change", renderControls);
$("load-profile-button").addEventListener("click", () => loadProfile($("profile-select").value));
$("save-profile-button").addEventListener("click", openSaveDialog);
$("save-cancel").addEventListener("click", () => $("save-dialog").close("cancel"));
$("save-dialog").addEventListener("cancel", (event) => { if (state.inFlight) event.preventDefault(); });
$("save-profile-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  if (state.inFlight || !state.saveContext) return;
  const name = $("profile-name").value.trim();
  if (!name) {
    text("profile-form-error", "请输入方案名称，不能仅包含空格。");
    shown("profile-form-error", true);
    $("profile-name").focus();
    return;
  }
  const { guid, mac } = state.saveContext;
  const adapterGuid = $("bind-profile").checked ? guid : null;
  const result = await execute("保存方案", async () => {
    acceptStore(await invoke("save_profile", { name, mac, adapterGuid }));
    toast("地址方案已保存，未修改网卡。");
  });
  if (result.ok) $("save-dialog").close("saved");
  else if (result.error) {
    text("profile-form-error", errorText(result.error));
    shown("profile-form-error", true);
  }
});
$("profile-search").addEventListener("input", renderProfiles);
$("profile-list").addEventListener("click", async (event) => {
  const button = event.target.closest("button[data-action]");
  if (!button || state.inFlight) return;
  const profile = state.snapshot?.store.profiles.find((item) => item.id === button.dataset.id);
  if (!profile) { showError("方案不存在，请刷新。"); return; }
  if (button.dataset.action === "copy") await copyProfile(profile);
  else if (button.dataset.action === "load") await loadProfile(profile.id);
  else if (button.dataset.action === "delete") await deleteProfile(profile);
});
$("import-button").addEventListener("click", () => { if (!state.inFlight) $("import-file").click(); });
$("import-file").addEventListener("change", async () => {
  const file = $("import-file").files?.[0];
  $("import-file").value = "";
  if (!file) return;
  if (state.inFlight) {
    showError("另一请求正在进行，请完成后重新选择导入文件。");
    return;
  }
  await execute("导入方案", async () => {
    if (file.size > 1024 * 1024) throw new Error("导入文件不能超过 1 MB。");
    const content = await file.text();
    // Keep parsing and merge rules in the backend. No frontend-only schema assumptions.
    acceptStore(await invoke("import_profiles", { content }));
    toast("已导入。");
  });
});
$("export-button").addEventListener("click", () => execute("导出方案", async () => {
  const content = await invoke("export_profiles");
  if (typeof content !== "string") throw new Error("后端返回的导出内容不是 JSON 文本。");
  JSON.parse(content);
  const url = URL.createObjectURL(new Blob([content], { type: "application/json;charset=utf-8" }));
  const link = element("a");
  link.href = url;
  link.download = "mac-address-profiles.json";
  link.hidden = true;
  document.body.append(link);
  try { link.click(); } finally { link.remove(); window.setTimeout(() => URL.revokeObjectURL(url), 1000); }
  toast("已生成 JSON 文件并请求下载。", "info");
}));
$("history-search").addEventListener("input", () => { state.historyPage = 1; renderHistory(); });
$("history-size").addEventListener("change", () => { state.historyPage = 1; renderHistory(); });
$("history-prev").addEventListener("click", () => { state.historyPage -= 1; renderHistory(); });
$("history-next").addEventListener("click", () => { state.historyPage += 1; renderHistory(); });
for (const id of ["monitor-input", "interval-input"]) $(id).addEventListener("change", () => {
  state.settingsDirty = true;
  renderSettings();
  renderControls();
});
$("settings-form").addEventListener("submit", (event) => {
  event.preventDefault();
  if (!state.settingsDirty) return;
  const settings = { monitor: $("monitor-input").checked, interval_seconds: Number($("interval-input").value) };
  execute("保存设置", async () => {
    const store = validateStore(await invoke("save_settings", { settings }));
    state.settingsDirty = false;
    acceptStore(store);
    toast("检测设置已保存。");
  });
});
$("elevate-button").addEventListener("click", async () => {
  if (state.inFlight || state.snapshot?.admin) return;
  const confirmed = await confirmAction({ title: "以管理员权限重新打开？", description: "Windows 将请求确认，未保存的编辑会丢失。", accept: "请求权限", destructive: false });
  if (!confirmed) return;
  await execute("请求管理员权限", async () => {
    await invoke("relaunch_admin");
    if (isDemo) { await fetchSnapshot(); toast("演示已切换为管理员，不涉及系统权限。", "info"); }
    else toast("已请求以管理员权限重新打开，请留意 Windows 权限确认。", "info");
  });
});
$("open-wifi-button").addEventListener("click", () => execute("打开 Wi-Fi 设置", async () => {
  await invoke("open_wifi_settings");
  toast(isDemo ? "演示环境。" : "已打开 Wi-Fi 设置。", "info");
}));
$("open-data-button").addEventListener("click", () => execute("打开数据目录", async () => {
  await invoke("open_data_folder");
  toast(isDemo ? "演示环境。" : "已打开数据目录。", "info");
}));
document.addEventListener("visibilitychange", schedulePoll);
window.addEventListener("pagehide", () => window.clearTimeout(state.pollTimer));
window.addEventListener("pageshow", schedulePoll);
window.addEventListener("unhandledrejection", (event) => showError(event.reason, "未完成的异步操作"));
window.addEventListener("error", (event) => { if (event.error) showError(event.error, "界面错误"); });

if (demo) {
  shown("demo-banner", true);
  $("demo-scenario").value = demo.scenario;
  $("demo-scenario").addEventListener("change", async () => {
    if (state.inFlight) return;
    demo.setScenario($("demo-scenario").value);
    state.snapshot = null;
    state.selectedGuid = null;
    state.drafts.clear();
    state.settingsDirty = false;
    state.uncertain = false;
    state.recoveryLatch = false;
    state.lastRead = null;
    state.historyPage = 1;
    for (const id of ["adapter-search", "profile-search", "history-search"]) $(id).value = "";
    $("include-virtual").checked = false;
    shown("error-banner", false);
    shown("operation-result", false);
    await initialLoad();
  });
}
async function initialLoad() {
  renderGlobal();
  renderControls();
  shown("adapter-detail", false);
  shown("no-selection", true);
  $("adapter-list").replaceChildren(emptyState("正在读取网卡", "稍候", "network"));
  $("profile-list").replaceChildren(emptyState("暂无方案", "读取后显示。", "profile"));
  text("profile-count", 0);
  text("adapter-count", "—");
  text("profiles-summary", "—");
  text("refresh-status", "正在读取");
  renderHistory();
  renderSettings();
  renderAbout();
  await fetchAppInfo();
  const result = await refresh(false);
  if (!result.ok && !state.snapshot) {
    $("adapter-list").replaceChildren(emptyState("读取失败", "查看错误后刷新。", "info"));
    text("refresh-status", "读取失败");
  }
}
initialLoad();
