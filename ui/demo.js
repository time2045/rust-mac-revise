// Browser-only simulation. No device APIs, disk writes, network calls, or real identifiers.
// Refreshing the browser or choosing a scenario resets this deterministic in-memory store.
const SCENARIOS = new Set(["normal", "readonly", "empty", "drift", "pending", "error", "failed", "rolled_back", "recovery_required", "write_pending"]);
const BASE_TIME = Date.parse("2026-01-15T08:00:00Z");
const clone = (value) => structuredClone(value);

function normalizeMac(value) {
  const raw = String(value || "").trim();
  if (!/^(?:[\da-f]{12}|(?:[\da-f]{2}-){5}[\da-f]{2}|(?:[\da-f]{2}:){5}[\da-f]{2})$/i.test(raw)) throw new Error("MAC 地址格式不正确。");
  const hex = raw.replace(/[:-]/g, "").toUpperCase();
  if (hex === "000000000000" || hex === "FFFFFFFFFFFF" || (Number.parseInt(hex.slice(0, 2), 16) & 1)) throw new Error("需要非零、非广播的单播地址。");
  return hex.match(/.{2}/g).join("-");
}
function sampleAdapters() {
  return [
    { guid: "DEMO-ETHERNET-001", name: "以太网", description: "演示千兆以太网适配器", pnp_id: "DEMO\\ETHERNET\\001", mac: "02-00-00-00-10-01", permanent_mac: "02-00-00-00-10-01", configured_mac: null, config_error: null, admin_enabled: true, status: "Up", hardware: true, can_configure: true },
    { guid: "DEMO-WIFI-002", name: "Wi-Fi", description: "演示无线网络适配器", pnp_id: "DEMO\\WIFI\\002", mac: "02-00-00-00-20-02", permanent_mac: null, configured_mac: null, config_error: null, admin_enabled: true, status: "Down", hardware: true, can_configure: true },
    { guid: "DEMO-USB-003", name: "USB 以太网", description: "演示 USB 网络适配器", pnp_id: "DEMO\\USB\\003", mac: null, permanent_mac: "02-00-00-00-30-03", configured_mac: null, config_error: null, admin_enabled: false, status: "Down", hardware: true, can_configure: true },
    { guid: "DEMO-VIRTUAL-004", name: "虚拟网络", description: "演示虚拟交换机适配器", pnp_id: "DEMO\\VIRTUAL\\004", mac: "02-00-00-00-40-04", permanent_mac: null, configured_mac: null, config_error: "此合成虚拟设备不提供地址覆盖配置。", admin_enabled: true, status: "Up", hardware: false, can_configure: false },
    { guid: "DEMO-VPN-005", name: "VPN 隧道", description: "演示隧道适配器", pnp_id: "DEMO\\VPN\\005", mac: null, permanent_mac: null, configured_mac: null, config_error: "隧道接口没有可配置的以太网地址。", admin_enabled: false, status: "Down", hardware: false, can_configure: false },
  ];
}
function initialStore() {
  const history = Array.from({ length: 24 }, (_, index) => {
    const outcomes = ["success", "pending", "failed", "rolled_back", "success", "success"];
    const outcome = outcomes[index % outcomes.length];
    return {
      id: `demo-history-${String(index + 1).padStart(3, "0")}`,
      time: new Date(BASE_TIME - (index + 1) * 3600000).toISOString(),
      adapter_guid: index % 2 ? "DEMO-WIFI-002" : "DEMO-ETHERNET-001",
      adapter_name: index % 2 ? "Wi-Fi" : "以太网",
      action: index % 5 === 0 ? "restore" : "apply",
      requested_mac: index % 5 === 0 ? null : "02-00-00-00-50-01",
      previous_mac: "02-00-00-00-10-01",
      actual_mac: outcome === "pending" ? null : outcome === "success" && index % 5 !== 0 ? "02-00-00-00-50-01" : "02-00-00-00-10-01",
      outcome,
      message: { success: "演示记录：操作完成，地址已验证。", pending: "演示记录：网卡保持禁用，待下次手动启用。", failed: "演示记录：驱动拒绝写入，配置未改变。", rolled_back: "演示记录：验证未通过，已回滚至上次配置。" }[outcome],
    };
  });
  return {
    version: 1,
    profiles: [
      { id: "demo-profile-office", name: "办公网络", mac: "02-00-00-00-50-01", adapter_guid: "DEMO-ETHERNET-001", created_at: "2026-01-12T02:30:00Z" },
      { id: "demo-profile-travel", name: "临时测试", mac: "02-00-00-00-50-02", adapter_guid: null, created_at: "2026-01-10T06:15:00Z" },
      { id: "demo-profile-wireless", name: "无线备用", mac: "02-00-00-00-50-03", adapter_guid: "DEMO-WIFI-002", created_at: "2026-01-08T10:00:00Z" },
    ],
    expectations: {},
    history,
    settings: { monitor: true, interval_seconds: 60, auto_repair: {} },
    pending: null,
  };
}
function macBytes(mac) { return mac ? mac.split("-").map((part) => Number.parseInt(part, 16)) : null; }
function bytesMac(bytes) { return bytes.map((byte) => byte.toString(16).padStart(2, "0").toUpperCase()).join("-"); }

export function createDemoBackend(search = "") {
  const params = new URLSearchParams(search);
  const outcomeParam = params.get("outcome");
  let initialScenario = params.get("scenario") || "normal";
  if (params.get("empty") === "1") initialScenario = "empty";
  else if (params.get("error") === "1") initialScenario = "error";
  else if (params.get("pending") === "1") initialScenario = "pending";
  else if (params.get("drift") === "1") initialScenario = "drift";
  else if (outcomeParam) initialScenario = outcomeParam === "pending" ? "write_pending" : outcomeParam;
  else if (params.get("admin") === "0") initialScenario = "readonly";
  let scenario;
  let snapshot;
  let sequence;
  let randomSequence;

  function now() { return new Date(BASE_TIME + (++sequence) * 60000).toISOString(); }
  function journal(adapter, requestedMac, action) {
    return {
      guid: adapter.guid,
      adapter_name: adapter.name,
      previous_override: adapter.configured_mac ? { bytes: macBytes(adapter.configured_mac) } : null,
      was_enabled: adapter.admin_enabled,
      previous_mac: adapter.mac,
      requested_mac: requestedMac,
      action,
    };
  }
  function setScenario(next) {
    scenario = SCENARIOS.has(next) ? next : "normal";
    sequence = 0;
    randomSequence = 0;
    snapshot = {
      adapters: sampleAdapters(),
      store: initialStore(),
      admin: scenario !== "readonly",
      data_dir: "演示数据 · 仅在内存中，没有磁盘目录",
      store_warning: null,
    };
    if (scenario === "empty") {
      snapshot.adapters = [];
      snapshot.store.profiles = [];
      snapshot.store.history = [];
    }
    if (scenario === "drift") {
      snapshot.store.expectations["DEMO-ETHERNET-001"] = "02-00-00-00-50-01";
      snapshot.adapters[0].configured_mac = "02-00-00-00-50-01";
    }
    if (scenario === "pending") {
      const adapter = snapshot.adapters[0];
      snapshot.store.pending = journal(adapter, "02-00-00-00-50-01", "apply");
      adapter.configured_mac = "02-00-00-00-50-01";
    }
    if (scenario === "write_pending") {
      snapshot.adapters[0].admin_enabled = false;
      snapshot.adapters[0].status = "Down";
      snapshot.adapters[0].mac = null;
    }
  }
  setScenario(initialScenario);
  // Query flags may be combined, e.g. ?pending=1&admin=0.
  if (params.get("admin") === "0") snapshot.admin = false;

  function getAdapter(guid) {
    const adapter = snapshot.adapters.find((item) => item.guid === guid);
    if (!adapter) throw new Error("演示网卡不存在或已移除。");
    return adapter;
  }
  function requireWrite(adapter, recovering = false) {
    if (!snapshot.admin) throw new Error("需要管理员权限。");
    if (!adapter.can_configure) throw new Error("此网卡不可配置。");
    if (snapshot.store.pending && !recovering) throw new Error("存在未完成操作，请先恢复上次配置。");
  }
  function record(adapter, action, requestedMac, previousMac, outcome, message) {
    const time = now();
    snapshot.store.history.unshift({ id: `demo-write-${sequence}`, time, adapter_guid: adapter.guid, adapter_name: adapter.name, action, requested_mac: requestedMac, previous_mac: previousMac, actual_mac: adapter.mac, outcome, message });
  }
  function write(guid, value, action) {
    const adapter = getAdapter(guid);
    requireWrite(adapter);
    const isApply = action === "apply" || action === "auto_repair";
    const requestedMac = isApply ? normalizeMac(value) : null;
    const before = clone(adapter);
    const pending = journal(adapter, requestedMac, action);
    let outcome;
    let message;
    if (scenario === "failed") {
      outcome = "failed";
      message = "演示：驱动拒绝配置，未改变原地址。可切换标准场景后重试。";
    } else if (scenario === "rolled_back") {
      outcome = "rolled_back";
      message = "演示：地址验证失败，已自动回滚至上次配置。";
    } else if (scenario === "recovery_required") {
      snapshot.store.pending = pending;
      adapter.configured_mac = requestedMac;
      outcome = "recovery_required";
      message = "演示：写入后验证中断，恢复日志已保留。请先恢复上次配置。";
    } else {
      adapter.configured_mac = requestedMac;
      const target = requestedMac || adapter.permanent_mac || "02-00-00-00-20-02";
      if (adapter.admin_enabled) {
        adapter.mac = target;
        outcome = "success";
        message = isApply ? "演示：地址覆盖已应用，实际地址已验证。" : "演示：已移除地址覆盖，当前使用驱动默认地址。";
      } else {
        outcome = "pending";
        message = "演示：配置已保存；网卡保持禁用，待下次手动启用后验证。";
      }
      if (action === "restore") {
        delete snapshot.store.expectations[guid];
        delete snapshot.store.settings.auto_repair[guid];
      } else {
        snapshot.store.expectations[guid] = target;
      }
      snapshot.store.pending = null;
    }
    record(adapter, action, requestedMac, before.mac, outcome, message);
    return { outcome, message };
  }
  function recover() {
    const pending = snapshot.store.pending;
    if (!pending) throw new Error("没有待恢复的操作。");
    const adapter = getAdapter(pending.guid);
    requireWrite(adapter, true);
    const previousMac = adapter.mac;
    adapter.configured_mac = pending.previous_override ? bytesMac(pending.previous_override.bytes) : null;
    adapter.admin_enabled = pending.was_enabled;
    adapter.status = pending.was_enabled ? "Up" : "Down";
    adapter.mac = pending.previous_mac;
    snapshot.store.pending = null;
    snapshot.store_warning = null;
    const outcome = pending.was_enabled ? "success" : "pending";
    const message = pending.was_enabled ? "演示：已恢复操作前的配置与设备启用状态。" : "演示：已恢复上次配置，网卡保持禁用，待下次手动启用验证。";
    const target = adapter.configured_mac || adapter.permanent_mac || adapter.mac;
    if (target) snapshot.store.expectations[adapter.guid] = target;
    record(adapter, "recover", target, previousMac, outcome, message);
    return { outcome, message };
  }
  function importProfiles(content) {
    if (typeof content !== "string") throw new Error("导入内容必须为 JSON 文本。");
    let parsed;
    try { parsed = JSON.parse(content.replace(/^\uFEFF/, "")); } catch { throw new Error("JSON 无法解析，请检查文件格式。"); }
    const input = Array.isArray(parsed) ? parsed : parsed?.profiles;
    if (!Array.isArray(input)) throw new Error("导入 JSON 需要包含 profiles 数组。");
    if (input.length > 10000) throw new Error("一次最多导入 10000 个方案。");
    // Validate every item before merging, so malformed imports never partially change state.
    const profiles = input.map((item, index) => {
      if (!item || typeof item.name !== "string" || !item.name.trim() || item.name.trim().length > 80) throw new Error(`第 ${index + 1} 个方案的名称无效。`);
      if (item.adapter_guid !== null && item.adapter_guid !== undefined && typeof item.adapter_guid !== "string") throw new Error(`第 ${index + 1} 个方案的绑定网卡无效。`);
      return { id: typeof item.id === "string" && item.id ? item.id : null, name: item.name.trim(), mac: normalizeMac(item.mac), adapter_guid: item.adapter_guid || null, created_at: typeof item.created_at === "string" && !Number.isNaN(Date.parse(item.created_at)) ? item.created_at : null };
    });
    for (const profile of profiles) {
      if (!profile.created_at) profile.created_at = now();
      if (!profile.id) profile.id = `demo-import-${++sequence}`;
      const existing = snapshot.store.profiles.findIndex((item) => item.id === profile.id);
      if (existing >= 0) snapshot.store.profiles[existing] = profile;
      else snapshot.store.profiles.push(profile);
    }
    return snapshot.store;
  }
  const commands = {
    get_snapshot() {
      if (scenario === "error") throw new Error("演示读取失败：无法访问设备快照。请选择其他演示场景以恢复。");
      return snapshot;
    },
    random_mac() {
      // Counter-based synthetic local/unicast addresses; reproducible across reloads.
      const suffix = (++randomSequence).toString(16).padStart(6, "0").toUpperCase().match(/.{2}/g).join("-");
      return `02-DA-7A-${suffix}`;
    },
    save_profile({ name, mac, adapterGuid }) {
      if (typeof name !== "string" || !name.trim() || name.trim().length > 80) throw new Error("方案名称需要 1–80 个字符。");
      if (adapterGuid !== null && adapterGuid !== undefined) getAdapter(adapterGuid);
      const normalized = normalizeMac(mac);
      const createdAt = now();
      snapshot.store.profiles.unshift({ id: `demo-saved-${sequence}`, name: name.trim(), mac: normalized, adapter_guid: adapterGuid || null, created_at: createdAt });
      return snapshot.store;
    },
    delete_profile({ id }) {
      if (!snapshot.store.profiles.some((profile) => profile.id === id)) throw new Error("方案不存在。");
      snapshot.store.profiles = snapshot.store.profiles.filter((profile) => profile.id !== id);
      return snapshot.store;
    },
    import_profiles({ content }) { return importProfiles(content); },
    export_profiles() { return JSON.stringify({ version: 1, profiles: snapshot.store.profiles }, null, 2); },
    save_settings({ settings }) {
      if (!settings || typeof settings.monitor !== "boolean" || ![30, 60, 120].includes(settings.interval_seconds)) throw new Error("检测设置无效。");
      snapshot.store.settings.monitor = settings.monitor;
      snapshot.store.settings.interval_seconds = settings.interval_seconds;
      return snapshot.store;
    },
    set_auto_repair({ guid, enabled }) {
      getAdapter(guid);
      if (enabled && !snapshot.store.expectations[guid]) throw new Error("请先应用地址。");
      if (enabled) snapshot.store.settings.auto_repair[guid] = true;
      else delete snapshot.store.settings.auto_repair[guid];
      return snapshot.store;
    },
    app_info() {
      return { version: "0.2.1", repository: "https://github.com/time2045/rust-mac-revise", releases: "https://github.com/time2045/rust-mac-revise/releases/latest" };
    },
    open_project_page() { return null; },
    apply_mac({ guid, mac }) { return write(guid, mac, "apply"); },
    auto_repair_mac({ guid, mac }) { return write(guid, mac, "auto_repair"); },
    restore_mac({ guid }) { return write(guid, null, "restore"); },
    recover_operation() { return recover(); },
    relaunch_admin() { snapshot.admin = true; return null; },
    open_wifi_settings() { return null; },
    open_data_folder() { return null; },
  };
  return {
    get scenario() { return scenario; },
    setScenario,
    async invoke(command, args = {}) {
      // Yield without timers so even synthetic operations exercise the UI's request gate.
      await Promise.resolve();
      if (!Object.prototype.hasOwnProperty.call(commands, command)) throw new Error(`未知演示命令：${command}`);
      return clone(commands[command](args));
    },
  };
}
