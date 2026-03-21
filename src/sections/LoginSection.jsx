import { useEffect, useMemo, useState } from "react";
import BilibiliLoginSection from "./BilibiliLoginSection";
import { invokeCommand } from "../lib/tauri";
import { formatDateTimeBeijing } from "../lib/format";

const parseBiliProfile = (authStatus) => {
  const raw = authStatus?.userInfo || {};
  const level1 = raw?.data || raw;
  const level2 = level1?.data || level1;
  const user = level2 || level1 || {};
  const nickname =
    user?.uname ||
    user?.name ||
    user?.nickname ||
    user?.username ||
    "未登录";
  const uid = user?.mid || user?.uid || user?.userId || user?.id || "";
  return {
    nickname,
    uid: uid ? String(uid) : "",
  };
};

export default function LoginSection({
  authStatus,
  onAuthChange,
  baiduStatus,
  onBaiduChange,
  onRefreshBaidu,
  onBindingChange,
  biliAddRequestKey = 0,
}) {
  const [activeTab, setActiveTab] = useState("bilibili");
  const [biliAddMode, setBiliAddMode] = useState(false);
  const [baiduLoginTab, setBaiduLoginTab] = useState("cookie");
  const [baiduForm, setBaiduForm] = useState({
    cookie: "",
    bduss: "",
    stoken: "",
  });
  const [baiduAccountForm, setBaiduAccountForm] = useState({
    username: "",
    password: "",
    input: "",
  });
  const [baiduAccountStatus, setBaiduAccountStatus] = useState({
    status: "IDLE",
    prompt: "",
    captchaPath: "",
    captchaUrl: "",
    output: [],
    lastError: "",
  });
  const [baiduAccountLoading, setBaiduAccountLoading] = useState(false);
  const [baiduMessage, setBaiduMessage] = useState("");
  const [baiduLoading, setBaiduLoading] = useState(false);
  const [biliMessage, setBiliMessage] = useState("");
  const [biliLoading, setBiliLoading] = useState(false);
  const [baiduAccounts, setBaiduAccounts] = useState([]);
  const [accountBindings, setAccountBindings] = useState({});

  const getErrorMessage = (error, fallback) => {
    if (!error) {
      return fallback;
    }
    if (typeof error === "string") {
      return error;
    }
    if (typeof error.message === "string" && error.message) {
      return error.message;
    }
    return fallback;
  };

  const biliProfile = useMemo(() => parseBiliProfile(authStatus), [authStatus]);
  const biliMeta = authStatus?.loginMeta || {};
  const biliLoginTime = biliMeta?.loginTime || "";
  const biliExpireTime = biliMeta?.expireTime || "";
  const biliAccounts = useMemo(
    () => (Array.isArray(authStatus?.accounts) ? authStatus.accounts : []),
    [authStatus],
  );

  const baiduLoggedIn = baiduStatus?.status === "LOGGED_IN";

  useEffect(() => {
    if (!biliAddRequestKey) {
      return;
    }
    setActiveTab("bilibili");
    setBiliAddMode(true);
  }, [biliAddRequestKey]);

  useEffect(() => {
    if (activeTab === "baidu") {
      onRefreshBaidu?.();
      loadBaiduAccounts();
    }
    if (activeTab === "bilibili") {
      loadBaiduAccounts();
      loadAccountBindings();
    }
  }, [activeTab, onRefreshBaidu]);

  useEffect(() => {
    if (activeTab === "baidu" && baiduLoginTab === "account") {
      loadBaiduAccountStatus();
    }
  }, [activeTab, baiduLoginTab]);

  useEffect(() => {
    if (activeTab !== "baidu" || baiduLoginTab !== "account") {
      return;
    }
    if (baiduAccountStatus.status === "SUCCESS") {
      onRefreshBaidu?.();
    }
  }, [activeTab, baiduLoginTab, baiduAccountStatus.status, onRefreshBaidu]);

  useEffect(() => {
    if (activeTab !== "baidu" || baiduLoginTab !== "account") {
      return undefined;
    }
    if (!["RUNNING", "WAIT_INPUT"].includes(baiduAccountStatus.status)) {
      return undefined;
    }
    const timer = setInterval(() => {
      loadBaiduAccountStatus();
    }, 1000);
    return () => clearInterval(timer);
  }, [activeTab, baiduLoginTab, baiduAccountStatus.status]);

  const loadBaiduAccountStatus = async () => {
    try {
      const data = await invokeCommand("baidu_sync_account_login_status");
      if (data) {
        setBaiduAccountStatus({
          status: data.status || "IDLE",
          prompt: data.prompt || "",
          captchaPath: data.captchaPath || "",
          captchaUrl: data.captchaUrl || "",
          output: Array.isArray(data.output) ? data.output : [],
          lastError: data.lastError || "",
        });
      }
    } catch (error) {
      setBaiduAccountStatus((prev) => ({
        ...prev,
        lastError: getErrorMessage(error, "读取登录状态失败"),
      }));
    }
  };

  const loadBaiduAccounts = async () => {
    try {
      const data = await invokeCommand("baidu_accounts_list");
      setBaiduAccounts(Array.isArray(data) ? data : []);
    } catch (_) {
      setBaiduAccounts([]);
    }
  };

  const loadAccountBindings = async () => {
    try {
      const data = await invokeCommand("account_binding_list");
      const next = {};
      (Array.isArray(data) ? data : []).forEach((item) => {
        const uid = String(item?.bilibiliUid || "");
        const baiduUid = String(item?.baiduUid || "");
        if (uid) {
          next[uid] = baiduUid;
        }
      });
      setAccountBindings(next);
      onBindingChange?.();
    } catch (_) {
      setAccountBindings({});
    }
  };

  const handleBaiduLogin = async () => {
    setBaiduMessage("");
    setBaiduLoading(true);
    try {
      await invokeCommand("auth_client_log", {
        message: `baidu_login_click type=${baiduLoginTab} cookie_len=${baiduForm.cookie.trim().length} bduss_len=${baiduForm.bduss.trim().length} stoken_len=${baiduForm.stoken.trim().length}`,
      });
      const payload = {
        loginType: baiduLoginTab,
        cookie: baiduForm.cookie,
        bduss: baiduForm.bduss,
        stoken: baiduForm.stoken,
      };
      const data = await invokeCommand("baidu_sync_login", { request: payload });
      onBaiduChange?.(data || { status: "LOGGED_OUT" });
      await loadBaiduAccounts();
      setBaiduMessage("登录成功");
    } catch (error) {
      const message = getErrorMessage(error, "登录失败");
      setBaiduMessage(message);
      await invokeCommand("auth_client_log", {
        message: `baidu_login_fail type=${baiduLoginTab} err=${message}`,
      });
    } finally {
      setBaiduLoading(false);
    }
  };

  const handleBaiduWebLogin = async () => {
    setBaiduMessage("请在弹窗内完成登录，若扫码无效可切换短信或密码登录");
    setBaiduLoading(true);
    try {
      const cookie = await invokeCommand("baidu_sync_web_login");
      setBaiduLoginTab("cookie");
      setBaiduForm((prev) => ({ ...prev, cookie }));
      const data = await invokeCommand("baidu_sync_login", {
        request: { loginType: "cookie", cookie },
      });
      onBaiduChange?.(data || { status: "LOGGED_OUT" });
      await loadBaiduAccounts();
      setBaiduMessage("网页登录登录成功");
    } catch (error) {
      setBaiduMessage(error?.message || "网页登录失败");
    } finally {
      setBaiduLoading(false);
    }
  };

  const handleBaiduAccountStart = async () => {
    setBaiduMessage("");
    setBaiduAccountLoading(true);
    try {
      const data = await invokeCommand("baidu_sync_account_login_start", {
        request: {
          username: baiduAccountForm.username,
          password: baiduAccountForm.password,
        },
      });
      setBaiduAccountStatus({
        status: data.status || "RUNNING",
        prompt: data.prompt || "",
        captchaPath: data.captchaPath || "",
        captchaUrl: data.captchaUrl || "",
        output: Array.isArray(data.output) ? data.output : [],
        lastError: data.lastError || "",
      });
    } catch (error) {
      setBaiduAccountStatus((prev) => ({
        ...prev,
        status: "FAILED",
        lastError: getErrorMessage(error, "启动登录失败"),
      }));
    } finally {
      setBaiduAccountLoading(false);
    }
  };

  const handleBaiduAccountInput = async () => {
    if (!baiduAccountForm.input.trim()) {
      return;
    }
    setBaiduAccountLoading(true);
    try {
      await invokeCommand("baidu_sync_account_login_input", {
        request: {
          input: baiduAccountForm.input,
        },
      });
      setBaiduAccountForm((prev) => ({ ...prev, input: "" }));
      await loadBaiduAccountStatus();
    } catch (error) {
      setBaiduAccountStatus((prev) => ({
        ...prev,
        lastError: getErrorMessage(error, "发送输入失败"),
      }));
    } finally {
      setBaiduAccountLoading(false);
    }
  };

  const handleBaiduAccountCancel = async () => {
    setBaiduAccountLoading(true);
    try {
      await invokeCommand("baidu_sync_account_login_cancel");
      await loadBaiduAccountStatus();
    } catch (error) {
      setBaiduAccountStatus((prev) => ({
        ...prev,
        lastError: getErrorMessage(error, "取消失败"),
      }));
    } finally {
      setBaiduAccountLoading(false);
    }
  };

  const handleOpenCaptchaPath = async () => {
    if (!baiduAccountStatus.captchaPath) {
      return;
    }
    try {
      const { openPath } = await import("@tauri-apps/plugin-opener");
      await openPath(baiduAccountStatus.captchaPath);
    } catch (error) {
      setBaiduAccountStatus((prev) => ({
        ...prev,
        lastError: getErrorMessage(error, "打开验证码失败"),
      }));
    }
  };

  const handleOpenCaptchaUrl = async () => {
    if (!baiduAccountStatus.captchaUrl) {
      return;
    }
    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(baiduAccountStatus.captchaUrl);
    } catch (error) {
      setBaiduAccountStatus((prev) => ({
        ...prev,
        lastError: getErrorMessage(error, "打开验证码链接失败"),
      }));
    }
  };

  const handleBaiduLogout = async () => {
    setBaiduMessage("");
    setBaiduLoading(true);
    try {
      await invokeCommand("baidu_sync_logout");
      onBaiduChange?.({ status: "LOGGED_OUT" });
      await loadBaiduAccounts();
      setBaiduMessage("已退出登录");
    } catch (error) {
      setBaiduMessage(error?.message || "退出失败");
    } finally {
      setBaiduLoading(false);
    }
  };

  const handleBaiduRefresh = async () => {
    setBaiduMessage("");
    setBaiduLoading(true);
    try {
      const data = await invokeCommand("baidu_sync_status");
      onBaiduChange?.(data || { status: "LOGGED_OUT" });
      await loadBaiduAccounts();
      setBaiduMessage("状态已刷新");
    } catch (error) {
      setBaiduMessage(error?.message || "刷新失败");
    } finally {
      setBaiduLoading(false);
    }
  };

  const handleBiliRefresh = async () => {
    setBiliMessage("");
    setBiliLoading(true);
    try {
      const data = await invokeCommand("auth_refresh");
      onAuthChange?.(data || { loggedIn: false });
      await loadAccountBindings();
      setBiliMessage("登录已刷新");
    } catch (error) {
      setBiliMessage(getErrorMessage(error, "刷新失败"));
    } finally {
      setBiliLoading(false);
    }
  };

  const handleBiliSwitch = async (userId) => {
    setBiliMessage("");
    setBiliLoading(true);
    try {
      const data = await invokeCommand("auth_account_switch", { userId });
      onAuthChange?.(data || { loggedIn: false });
      await loadAccountBindings();
      setBiliMessage("已切换当前账号");
    } catch (error) {
      setBiliMessage(getErrorMessage(error, "切换失败"));
    } finally {
      setBiliLoading(false);
    }
  };

  const handleBiliLogout = async (userId) => {
    setBiliMessage("");
    setBiliLoading(true);
    try {
      const data = await invokeCommand("auth_account_logout", { userId });
      onAuthChange?.(data || { loggedIn: false });
      await loadAccountBindings();
      setBiliMessage("账号已退出");
    } catch (error) {
      setBiliMessage(getErrorMessage(error, "退出失败"));
    } finally {
      setBiliLoading(false);
    }
  };

  const handleBaiduSwitch = async (uid) => {
    setBaiduMessage("");
    setBaiduLoading(true);
    try {
      const data = await invokeCommand("baidu_account_switch", { uid });
      onBaiduChange?.(data || { status: "LOGGED_OUT" });
      await loadBaiduAccounts();
      setBaiduMessage("已切换当前网盘账号");
    } catch (error) {
      setBaiduMessage(getErrorMessage(error, "切换失败"));
    } finally {
      setBaiduLoading(false);
    }
  };

  const handleBaiduAccountLogout = async (uid) => {
    setBaiduMessage("");
    setBaiduLoading(true);
    try {
      await invokeCommand("baidu_account_logout", { uid });
      await loadBaiduAccounts();
      await loadAccountBindings();
      const data = await invokeCommand("baidu_sync_status");
      onBaiduChange?.(data || { status: "LOGGED_OUT" });
      setBaiduMessage("网盘账号已退出");
    } catch (error) {
      setBaiduMessage(getErrorMessage(error, "退出失败"));
    } finally {
      setBaiduLoading(false);
    }
  };

  const handleBindingSave = async (bilibiliUid) => {
    const selectedBaiduUid = String(accountBindings[String(bilibiliUid)] || "").trim();
    if (!selectedBaiduUid) {
      setBiliMessage("请选择要绑定的网盘账号");
      return;
    }
    setBiliLoading(true);
    setBiliMessage("");
    try {
      await invokeCommand("account_binding_set", {
        bilibiliUid,
        baiduUid: selectedBaiduUid,
      });
      await loadAccountBindings();
      onBindingChange?.();
      setBiliMessage("账号绑定已保存");
    } catch (error) {
      setBiliMessage(getErrorMessage(error, "保存绑定失败"));
    } finally {
      setBiliLoading(false);
    }
  };

  return (
    <div className="space-y-6">
      <div className="panel flex flex-wrap gap-3 p-2">
        <button
          className={activeTab === "bilibili" ? "tab-btn active" : "tab-btn"}
          onClick={() => setActiveTab("bilibili")}
        >
          Bilibili
        </button>
        <button
          className={activeTab === "baidu" ? "tab-btn active" : "tab-btn"}
          onClick={() => setActiveTab("baidu")}
        >
          百度网盘
        </button>
      </div>

      {activeTab === "bilibili" ? (
        <div className="space-y-4">
          <div className="panel p-4">
            <div className="flex items-center justify-between">
              <div className="text-sm font-semibold text-[var(--content-color)]">
                {biliAddMode ? "新增账号" : "登录信息"}
              </div>
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={handleBiliRefresh}
                disabled={biliLoading}
              >
                刷新登录
              </button>
            </div>
            {biliAddMode ? (
              <div className="mt-3 rounded-lg bg-white/70 px-4 py-4 text-sm text-[var(--content-color)]">
                <div className="font-semibold">当前正在添加新的 Bilibili 账号</div>
                <div className="mt-2 text-xs text-[var(--desc-color)]">
                  扫码成功后会自动判断是否为重复账号：
                  同一账号不会重复添加，已有账号会刷新状态并切换为当前账号。
                </div>
              </div>
            ) : (
              <div className="mt-3 grid gap-3 text-sm text-[var(--content-color)] md:grid-cols-2">
                <div className="rounded-lg bg-white/70 px-3 py-2">
                  <div className="text-xs text-[var(--desc-color)]">昵称</div>
                  <div className="font-semibold">{biliProfile.nickname}</div>
                </div>
                <div className="rounded-lg bg-white/70 px-3 py-2">
                  <div className="text-xs text-[var(--desc-color)]">UID</div>
                  <div className="font-semibold">{biliProfile.uid || "—"}</div>
                </div>
                <div className="rounded-lg bg-white/70 px-3 py-2">
                  <div className="text-xs text-[var(--desc-color)]">登录时间</div>
                  <div className="font-semibold">
                    {biliLoginTime ? formatDateTimeBeijing(biliLoginTime) : "—"}
                  </div>
                </div>
                <div className="rounded-lg bg-white/70 px-3 py-2">
                  <div className="text-xs text-[var(--desc-color)]">过期时间</div>
                  <div className="font-semibold">
                    {biliExpireTime ? formatDateTimeBeijing(biliExpireTime) : "—"}
                  </div>
                </div>
              </div>
            )}
            {biliMessage ? (
              <div className="mt-2 text-xs text-[var(--desc-color)]">{biliMessage}</div>
            ) : null}
            {!biliAddMode ? (
              <div className="mt-4 border-t border-black/5 pt-4">
                <div className="text-xs font-semibold text-[var(--desc-color)]">已登录账号</div>
                <div className="mt-3 space-y-2">
                  {biliAccounts.length > 0 ? (
                    biliAccounts.map((account) => (
                      <div
                        key={account.userId}
                        className="rounded-lg bg-white/70 px-3 py-2"
                      >
                        <div className="flex flex-wrap items-center justify-between gap-2">
                          <div className="text-sm text-[var(--content-color)]">
                            <span className="font-semibold">
                              {account.nickname || account.username || `UID ${account.userId}`}
                            </span>
                            <span className="ml-2 text-xs text-[var(--desc-color)]">
                              UID {account.userId}
                            </span>
                            {account.isActive ? (
                              <span className="ml-2 rounded-full bg-[var(--primary-color)]/10 px-2 py-0.5 text-xs text-[var(--primary-color)]">
                                当前
                              </span>
                            ) : null}
                          </div>
                          <div className="flex gap-2">
                            {!account.isActive ? (
                              <button
                                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                                onClick={() => handleBiliSwitch(account.userId)}
                                disabled={biliLoading}
                              >
                                切换
                              </button>
                            ) : null}
                            <button
                              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                              onClick={() => handleBiliLogout(account.userId)}
                              disabled={biliLoading}
                            >
                              退出
                            </button>
                          </div>
                        </div>
                        <div className="mt-3 flex flex-wrap items-center gap-2">
                          <select
                            value={accountBindings[String(account.userId)] || ""}
                            onChange={(event) =>
                              setAccountBindings((prev) => ({
                                ...prev,
                                [String(account.userId)]: event.target.value,
                              }))
                            }
                            className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                          >
                            <option value="">绑定网盘账号</option>
                            {baiduAccounts.map((baiduAccount) => (
                              <option key={baiduAccount.uid} value={baiduAccount.uid}>
                                {baiduAccount.username || baiduAccount.uid}
                              </option>
                            ))}
                          </select>
                          <button
                            className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                            onClick={() => handleBindingSave(account.userId)}
                            disabled={biliLoading}
                          >
                            保存绑定
                          </button>
                        </div>
                      </div>
                    ))
                  ) : (
                    <div className="rounded-lg bg-white/70 px-3 py-2 text-sm text-[var(--desc-color)]">
                      暂无已登录账号
                    </div>
                  )}
                </div>
              </div>
            ) : null}
          </div>
          <BilibiliLoginSection
            onStatusChange={onAuthChange}
            embedded
            initialStatus={authStatus}
            addRequestKey={biliAddRequestKey}
            onAddModeChange={setBiliAddMode}
          />
        </div>
      ) : (
        <div className="space-y-4">
          <div className="panel p-4">
            <div className="flex items-center justify-between">
              <div className="text-sm font-semibold text-[var(--content-color)]">登录信息</div>
              <div className="flex gap-2">
                <button
                  className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={handleBaiduRefresh}
                  disabled={baiduLoading}
                >
                  刷新
                </button>
                {baiduLoggedIn ? (
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                    onClick={handleBaiduLogout}
                    disabled={baiduLoading}
                  >
                    退出
                  </button>
                ) : null}
              </div>
            </div>
            <div className="mt-3 grid gap-3 text-sm text-[var(--content-color)] md:grid-cols-2">
              <div className="rounded-lg bg-white/70 px-3 py-2">
                <div className="text-xs text-[var(--desc-color)]">状态</div>
                <div className="font-semibold">
                  {baiduLoggedIn ? "已登录" : "未登录"}
                </div>
              </div>
              <div className="rounded-lg bg-white/70 px-3 py-2">
                <div className="text-xs text-[var(--desc-color)]">用户名</div>
                <div className="font-semibold">{baiduStatus?.username || "—"}</div>
              </div>
              <div className="rounded-lg bg-white/70 px-3 py-2">
                <div className="text-xs text-[var(--desc-color)]">UID</div>
                <div className="font-semibold">{baiduStatus?.uid || "—"}</div>
              </div>
              <div className="rounded-lg bg-white/70 px-3 py-2">
                <div className="text-xs text-[var(--desc-color)]">登录方式</div>
                <div className="font-semibold">
                  {baiduStatus?.loginType || "—"}
                </div>
              </div>
              <div className="rounded-lg bg-white/70 px-3 py-2">
                <div className="text-xs text-[var(--desc-color)]">登录时间</div>
                <div className="font-semibold">
                  {baiduStatus?.loginTime
                    ? formatDateTimeBeijing(baiduStatus?.loginTime)
                    : "—"}
                </div>
              </div>
              <div className="rounded-lg bg-white/70 px-3 py-2">
                <div className="text-xs text-[var(--desc-color)]">最后验证</div>
                <div className="font-semibold">
                  {baiduStatus?.lastCheckTime
                    ? formatDateTimeBeijing(baiduStatus?.lastCheckTime)
                    : "—"}
                </div>
              </div>
            </div>
            <div className="mt-4 border-t border-black/5 pt-4">
              <div className="text-xs font-semibold text-[var(--desc-color)]">已登录网盘账号</div>
              <div className="mt-3 space-y-2">
                {baiduAccounts.length > 0 ? (
                  baiduAccounts.map((account) => (
                    <div
                      key={account.uid}
                      className="flex flex-wrap items-center justify-between gap-2 rounded-lg bg-white/70 px-3 py-2"
                    >
                      <div className="text-sm text-[var(--content-color)]">
                        <span className="font-semibold">{account.username || account.uid}</span>
                        <span className="ml-2 text-xs text-[var(--desc-color)]">
                          UID {account.uid}
                        </span>
                        {account.isActive ? (
                          <span className="ml-2 rounded-full bg-[var(--primary-color)]/10 px-2 py-0.5 text-xs text-[var(--primary-color)]">
                            当前
                          </span>
                        ) : null}
                      </div>
                      <div className="flex gap-2">
                        {!account.isActive ? (
                          <button
                            className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                            onClick={() => handleBaiduSwitch(account.uid)}
                            disabled={baiduLoading}
                          >
                            切换
                          </button>
                        ) : null}
                        <button
                          className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                          onClick={() => handleBaiduAccountLogout(account.uid)}
                          disabled={baiduLoading}
                        >
                          退出
                        </button>
                      </div>
                    </div>
                  ))
                ) : (
                  <div className="rounded-lg bg-white/70 px-3 py-2 text-sm text-[var(--desc-color)]">
                    暂无已登录网盘账号
                  </div>
                )}
              </div>
            </div>
          </div>

          {!baiduLoggedIn ? (
            <div className="panel p-4">
              <div className="text-sm font-semibold text-[var(--content-color)]">登录百度网盘</div>
              <div className="mt-3 flex gap-4 text-sm font-semibold text-[var(--desc-color)]">
                <button
                  className={baiduLoginTab === "cookie" ? "text-[var(--primary-color)]" : ""}
                  onClick={() => setBaiduLoginTab("cookie")}
                >
                  Cookie 登录
                </button>
                <button
                  className={baiduLoginTab === "bduss" ? "text-[var(--primary-color)]" : ""}
                  onClick={() => setBaiduLoginTab("bduss")}
                >
                  BDUSS 登录
                </button>
                <button
                  className={baiduLoginTab === "account" ? "text-[var(--primary-color)]" : ""}
                  onClick={() => setBaiduLoginTab("account")}
                >
                  账号登录
                </button>
              </div>
              {baiduLoginTab === "cookie" ? (
                <div className="mt-4 flex flex-col gap-3 text-sm">
                  <textarea
                    value={baiduForm.cookie}
                    onChange={(event) =>
                      setBaiduForm((prev) => ({ ...prev, cookie: event.target.value }))
                    }
                    placeholder="请输入 Cookie（包含 BDUSS 等字段）"
                    className="min-h-[120px] rounded-lg border border-black/10 bg-white/80 px-3 py-2"
                  />
                </div>
              ) : baiduLoginTab === "bduss" ? (
                <div className="mt-4 grid gap-3 text-sm md:grid-cols-2">
                  <div className="flex flex-col gap-2">
                    <span className="text-xs text-[var(--desc-color)]">BDUSS</span>
                    <input
                      value={baiduForm.bduss}
                      onChange={(event) =>
                        setBaiduForm((prev) => ({ ...prev, bduss: event.target.value }))
                      }
                      placeholder="请输入 BDUSS"
                      className="rounded-lg border border-black/10 bg-white/80 px-3 py-2"
                    />
                  </div>
                  <div className="flex flex-col gap-2">
                    <span className="text-xs text-[var(--desc-color)]">STOKEN</span>
                    <input
                      value={baiduForm.stoken}
                      onChange={(event) =>
                        setBaiduForm((prev) => ({ ...prev, stoken: event.target.value }))
                      }
                      placeholder="可选"
                      className="rounded-lg border border-black/10 bg-white/80 px-3 py-2"
                    />
                  </div>
                </div>
              ) : (
                <div className="mt-4 space-y-3 text-sm">
                  <div className="grid gap-3 md:grid-cols-2">
                    <div className="flex flex-col gap-2">
                      <span className="text-xs text-[var(--desc-color)]">账号</span>
                      <input
                        value={baiduAccountForm.username}
                        onChange={(event) =>
                          setBaiduAccountForm((prev) => ({
                            ...prev,
                            username: event.target.value,
                          }))
                        }
                        placeholder="手机号/邮箱/用户名"
                        className="rounded-lg border border-black/10 bg-white/80 px-3 py-2"
                      />
                    </div>
                    <div className="flex flex-col gap-2">
                      <span className="text-xs text-[var(--desc-color)]">密码</span>
                      <input
                        type="password"
                        value={baiduAccountForm.password}
                        onChange={(event) =>
                          setBaiduAccountForm((prev) => ({
                            ...prev,
                            password: event.target.value,
                          }))
                        }
                        placeholder="请输入密码"
                        className="rounded-lg border border-black/10 bg-white/80 px-3 py-2"
                      />
                    </div>
                  </div>
                  <div className="flex flex-wrap items-center gap-2">
                    <button
                      className="rounded-full bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white shadow-sm"
                      onClick={handleBaiduAccountStart}
                      disabled={baiduAccountLoading}
                    >
                      {baiduAccountLoading ? "登录中..." : "开始登录"}
                    </button>
                    <button
                      className="rounded-full border border-black/10 bg-white px-4 py-2 text-sm font-semibold text-[var(--ink)]"
                      onClick={handleBaiduAccountCancel}
                      disabled={baiduAccountLoading}
                    >
                      取消
                    </button>
                    <span className="text-xs text-[var(--desc-color)]">
                      状态：{baiduAccountStatus.status}
                    </span>
                  </div>
                  {baiduAccountStatus.captchaPath ? (
                    <div className="flex flex-wrap items-center gap-2 text-xs text-[var(--desc-color)]">
                      <span>验证码路径：{baiduAccountStatus.captchaPath}</span>
                      <button
                        className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                        onClick={handleOpenCaptchaPath}
                      >
                        打开验证码
                      </button>
                    </div>
                  ) : null}
                  {baiduAccountStatus.captchaUrl ? (
                    <div className="flex flex-wrap items-center gap-2 text-xs text-[var(--desc-color)]">
                      <span>验证码链接：{baiduAccountStatus.captchaUrl}</span>
                      <button
                        className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                        onClick={handleOpenCaptchaUrl}
                      >
                        打开链接
                      </button>
                    </div>
                  ) : null}
                  {baiduAccountStatus.prompt ? (
                    <div className="flex flex-wrap items-center gap-2">
                      <input
                        value={baiduAccountForm.input}
                        onChange={(event) =>
                          setBaiduAccountForm((prev) => ({
                            ...prev,
                            input: event.target.value,
                          }))
                        }
                        placeholder={baiduAccountStatus.prompt}
                        className="min-w-[220px] rounded-lg border border-black/10 bg-white/80 px-3 py-2"
                      />
                      <button
                        className="rounded-full border border-black/10 bg-white px-4 py-2 text-sm font-semibold text-[var(--ink)]"
                        onClick={handleBaiduAccountInput}
                        disabled={baiduAccountLoading}
                      >
                        发送
                      </button>
                    </div>
                  ) : null}
                  {baiduAccountStatus.output?.length ? (
                    <div className="max-h-48 overflow-y-auto rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-xs font-mono text-[var(--content-color)]">
                      {baiduAccountStatus.output.map((line, index) => (
                        <div key={`${line}-${index}`}>{line}</div>
                      ))}
                    </div>
                  ) : null}
                  {baiduAccountStatus.lastError ? (
                    <div className="rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-700">
                      {baiduAccountStatus.lastError}
                    </div>
                  ) : null}
                </div>
              )}
              {baiduLoginTab !== "account" ? (
                <div className="mt-4 flex gap-2">
                  <button
                    className="rounded-full bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white shadow-sm"
                    onClick={handleBaiduLogin}
                    disabled={baiduLoading}
                  >
                    {baiduLoading ? "登录中..." : "登录"}
                  </button>
                  <button
                    className="rounded-full border border-black/10 bg-white px-4 py-2 text-sm font-semibold text-[var(--ink)]"
                    onClick={handleBaiduWebLogin}
                    disabled={baiduLoading}
                  >
                    网页登录
                  </button>
                </div>
              ) : null}
            </div>
          ) : null}

          {baiduMessage ? (
            <div className="rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
              {baiduMessage}
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
}
