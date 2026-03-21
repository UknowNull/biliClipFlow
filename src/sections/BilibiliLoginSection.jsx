import { useEffect, useMemo, useRef, useState } from "react";
import { invokeCommand } from "../lib/tauri";

const DEFAULT_AVATAR = "https://i0.hdslb.com/bfs/face/member/noface.jpg";

const defaultUser = {
  avatar: DEFAULT_AVATAR,
  name: "Bilibili 用户",
  desc: "暂无签名",
  stat: {
    following: 0,
    follower: 0,
    dynamic: 0,
    coins: 0,
  },
};

const statusTextMap = {
  "-2": "加载中...",
  "86101": "等待扫描...",
  "86090": "扫码成功，请在手机上确认",
  "86038": "二维码已过期",
};

export default function BilibiliLoginSection({
  onStatusChange,
  embedded = false,
  initialStatus = null,
  addRequestKey = 0,
  onAddModeChange,
}) {
  const [isLogin, setIsLogin] = useState(Boolean(initialStatus?.loggedIn));
  const [scanStatus, setScanStatus] = useState(-1);
  const [qrData, setQrData] = useState(null);
  const [user, setUser] = useState(defaultUser);
  const [message, setMessage] = useState("");
  const [avatarPreview, setAvatarPreview] = useState("");
  const [forceScanMode, setForceScanMode] = useState(false);
  const pollRef = useRef(null);
  const forceScanModeRef = useRef(false);
  const addContextRef = useRef({
    previousActiveUserId: 0,
    previousAccountIds: [],
  });

  const qrImageSrc = useMemo(() => {
    if (!qrData?.url) {
      return "";
    }
    const encoded = encodeURIComponent(qrData.url);
    return `https://api.qrserver.com/v1/create-qr-code/?size=220x220&data=${encoded}`;
  }, [qrData]);

  const stopPolling = () => {
    if (pollRef.current) {
      clearInterval(pollRef.current);
      pollRef.current = null;
    }
  };

  const parseUserInfo = (status) => {
    const raw = status?.userInfo || {};
    const level1 = raw?.data || raw;
    const level2 = level1?.data || level1;
    const name =
      level2?.uname ||
      level2?.username ||
      level2?.name ||
      level1?.uname ||
      level1?.username ||
      level1?.name ||
      "Bilibili 用户";
    const avatar =
      level2?.avatar ||
      level2?.face ||
      level1?.avatar ||
      level1?.face ||
      DEFAULT_AVATAR;
    const stat = level2?.stat || level1?.stat || defaultUser.stat;
    return {
      ...defaultUser,
      name,
      avatar,
      desc: level2?.sign || level2?.desc || defaultUser.desc,
      stat: {
        following: stat?.following ?? defaultUser.stat.following,
        follower: stat?.follower ?? defaultUser.stat.follower,
        dynamic: stat?.dynamic ?? defaultUser.stat.dynamic,
        coins:
          stat?.coins ??
          level2?.coins ??
          level1?.coins ??
          defaultUser.stat.coins,
      },
    };
  };

  const refreshStatus = async () => {
    try {
      const data = await invokeCommand("auth_status");
      const loggedIn = Boolean(data?.loggedIn);
      setIsLogin(loggedIn);
      if (loggedIn) {
        setUser(parseUserInfo(data));
        setMessage("");
      }
      if (onStatusChange) {
        onStatusChange(data || { loggedIn: false });
      }
      return data || { loggedIn: false };
    } catch (error) {
      setMessage(error.message);
      return null;
    }
  };

  const pollStatus = async (qrcodeKey) => {
    try {
      await invokeCommand("auth_client_log", { message: `poll_tick:${String(qrcodeKey).length}` });
      const data = await invokeCommand("auth_qrcode_poll", { qrcodeKey });
      const code = data?.code ?? 86101;
      setScanStatus(code);
      if (code === 0) {
        stopPolling();
        const status = await refreshStatus();
        if (forceScanModeRef.current) {
          const nextUserId = Number(status?.activeAccount?.userId || 0);
          const previousActiveUserId = Number(addContextRef.current.previousActiveUserId || 0);
          const previousAccountIds = new Set(addContextRef.current.previousAccountIds || []);
          if (nextUserId > 0 && nextUserId === previousActiveUserId) {
            setMessage("当前已登录该账号，无需重复添加");
          } else if (nextUserId > 0 && previousAccountIds.has(nextUserId)) {
            setMessage("该账号已存在，已刷新登录状态并切换为当前账号");
          } else {
            setMessage("新账号已添加并切换为当前账号");
          }
          forceScanModeRef.current = false;
          setForceScanMode(false);
        }
        return;
      }
      if (code === 86038) {
        stopPolling();
      }
    } catch (error) {
      const raw = typeof error === "string" ? error : String(error || "");
      const message = error?.message || raw || "轮询失败";
      setMessage(message);
      try {
        await invokeCommand("auth_client_log", {
          message: `poll_error:${message}|raw:${raw}`,
        });
      } catch (_) {
        // ignore
      }
    }
  };

  const initScan = async () => {
    stopPolling();
    setMessage("");
    setScanStatus(-2);
    setQrData(null);
    try {
      const data = await invokeCommand("auth_qrcode_generate");
      if (!data?.url || !data?.qrcode_key) {
        throw new Error("二维码生成失败");
      }
      setQrData({ url: data.url, key: data.qrcode_key });
      setScanStatus(86101);
      pollRef.current = setInterval(() => {
        pollStatus(data.qrcode_key);
      }, 3000);
      await invokeCommand("auth_client_log", { message: `poll_start:${data.qrcode_key.length}` });
    } catch (error) {
      setMessage(error.message);
    }
  };

  const beginAddAccountFlow = async () => {
    let auth = initialStatus;
    if (!auth?.loggedIn) {
      try {
        auth = await invokeCommand("auth_status");
      } catch (_) {
        auth = initialStatus;
      }
    }
    const accountIds = Array.isArray(auth?.accounts)
      ? auth.accounts
          .map((item) => Number(item?.userId || 0))
          .filter((value) => Number.isFinite(value) && value > 0)
      : [];
    addContextRef.current = {
      previousActiveUserId: Number(auth?.activeAccount?.userId || 0),
      previousAccountIds: accountIds,
    };
    setForceScanMode(true);
    await initScan();
  };

  const loadAvatar = async (url) => {
    if (!url) {
      setAvatarPreview("");
      return;
    }
    setAvatarPreview(url);
    try {
      const data = await invokeCommand("video_proxy_image", { url });
      if (data) {
        setAvatarPreview(data);
      }
      await invokeCommand("auth_client_log", {
        message: `avatar_proxy_ok:${String(url).length}:${String(data || "").length}`,
      });
    } catch (error) {
      const message = error?.message || String(error || "");
      await invokeCommand("auth_client_log", {
        message: `avatar_proxy_fail:${String(url).length}:${message}`,
      });
    }
  };

  const handleLogout = async () => {
    setMessage("");
    try {
      await invokeCommand("auth_logout");
      setIsLogin(false);
      setUser(defaultUser);
      if (onStatusChange) {
        onStatusChange({ loggedIn: false });
      }
      initScan();
    } catch (error) {
      setMessage(error.message);
    }
  };

  useEffect(() => {
    if (initialStatus?.loggedIn) {
      setIsLogin(true);
      setUser(parseUserInfo(initialStatus));
      setMessage("");
    }
  }, [initialStatus]);

  useEffect(() => {
    const init = async () => {
      if (initialStatus?.loggedIn) {
        await refreshStatus();
        return;
      }
      const status = await refreshStatus();
      if (!status?.loggedIn) {
        initScan();
      }
    };
    init();
    return () => stopPolling();
  }, [initialStatus?.loggedIn]);

  useEffect(() => {
    forceScanModeRef.current = forceScanMode;
  }, [forceScanMode]);

  useEffect(() => {
    if (!addRequestKey) {
      return;
    }
    beginAddAccountFlow();
  }, [addRequestKey]);

  useEffect(() => {
    onAddModeChange?.(forceScanMode);
  }, [forceScanMode, onAddModeChange]);

  useEffect(() => {
    if (isLogin) {
      loadAvatar(user.avatar);
    } else {
      setAvatarPreview("");
    }
  }, [isLogin, user.avatar]);

  useEffect(() => {
    if (!isLogin) {
      return;
    }
    invokeCommand("auth_client_log", {
      message: `avatar_preview_update:${String(avatarPreview).length}`,
    }).catch(() => {});
    if (!avatarPreview) {
      invokeCommand("auth_client_log", {
        message: "avatar_fallback_render",
      }).catch(() => {});
    }
  }, [avatarPreview, isLogin]);

  const showScanPanel = forceScanMode || !isLogin;
  const titleText = forceScanMode ? "新增账号扫码" : "扫码登录";

  return (
    <div className={embedded ? "" : "space-y-4"}>
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-semibold text-[var(--content-color)]">{titleText}</h1>
        <div className="flex items-center gap-2">
          {isLogin && !forceScanMode ? (
            <button className="h-8 px-3 rounded-lg" onClick={beginAddAccountFlow}>
              新增账号
            </button>
          ) : null}
          {forceScanMode ? (
            <button
              className="h-8 px-3 rounded-lg"
              onClick={() => {
                stopPolling();
                setForceScanMode(false);
                setMessage("");
              }}
            >
              取消
            </button>
          ) : null}
          {isLogin && !forceScanMode ? (
            <button className="h-8 px-3 rounded-lg" onClick={handleLogout}>
              退出登录
            </button>
          ) : null}
        </div>
      </div>

      {showScanPanel ? (
        <div className="panel flex flex-col gap-6 p-4">
          <div className="flex flex-col items-center gap-3">
            <div className="text-base font-semibold text-[var(--content-color)]">{titleText}</div>
            <div className="relative flex h-48 w-48 items-center justify-center rounded-lg bg-white">
              {qrImageSrc ? (
                <img src={qrImageSrc} alt="二维码" className="h-40 w-40" />
              ) : (
                <div className="text-sm text-[var(--desc-color)]">二维码加载中...</div>
              )}
              {scanStatus === -2 ? (
                <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-white/80 text-sm text-[var(--desc-color)]">
                  <div className="h-6 w-6 animate-spin rounded-full border-2 border-[var(--primary-color)] border-t-transparent" />
                  <span>加载中...</span>
                </div>
              ) : null}
              {scanStatus === 86038 || scanStatus === 86090 ? (
                <button
                  className="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-white/90 text-sm text-[var(--desc-color)]"
                  onClick={initScan}
                >
                  <span>{scanStatus === 86038 ? "二维码已过期" : "扫码成功"}</span>
                  <span>{scanStatus === 86038 ? "点击刷新" : "请在手机上确认"}</span>
                </button>
              ) : null}
            </div>
            <div className="text-xs text-[var(--desc-color)]">请使用 Bilibili 客户端扫码</div>
            <div className="text-sm font-semibold text-[var(--content-color)]">
              {statusTextMap[String(scanStatus)] || "等待扫描..."}
            </div>
            <div className="max-w-md text-center text-xs text-[var(--desc-color)]">
              登录即代表你同意数据使用规则
            </div>
          </div>
        </div>
      ) : (
        <div className="panel p-4">
          <div className="h-24 w-full rounded-lg bg-gradient-to-r from-sky-200/70 to-slate-200/70" />
          <div className="-mt-8 flex flex-wrap items-center gap-4">
            <div className="h-16 w-16 overflow-hidden rounded-full border-4 border-white bg-white">
              {avatarPreview ? (
                <img
                  src={avatarPreview}
                  alt="用户头像"
                  className="h-full w-full object-cover"
                  onError={() => {
                    invokeCommand("auth_client_log", {
                      message: `avatar_img_error:${String(avatarPreview).length}`,
                    }).catch(() => {});
                    if (!String(avatarPreview).startsWith("data:")) {
                      setAvatarPreview("");
                    }
                  }}
                />
              ) : (
                <span className="avatar-fallback" />
              )}
            </div>
            <div>
              <div className="text-lg font-semibold text-[var(--content-color)]">{user.name}</div>
              <div className="text-xs text-[var(--desc-color)]">{user.desc}</div>
            </div>
          </div>
          <div className="mt-4 grid gap-3 text-sm text-[var(--content-color)] sm:grid-cols-4">
            <div className="rounded-lg bg-white/80 px-3 py-2 text-center">
              <div className="text-xs text-[var(--desc-color)]">关注数</div>
              <div className="font-semibold">{user.stat.following}</div>
            </div>
            <div className="rounded-lg bg-white/80 px-3 py-2 text-center">
              <div className="text-xs text-[var(--desc-color)]">粉丝数</div>
              <div className="font-semibold">{user.stat.follower}</div>
            </div>
            <div className="rounded-lg bg-white/80 px-3 py-2 text-center">
              <div className="text-xs text-[var(--desc-color)]">动态数</div>
              <div className="font-semibold">{user.stat.dynamic}</div>
            </div>
            <div className="rounded-lg bg-white/80 px-3 py-2 text-center">
              <div className="text-xs text-[var(--desc-color)]">硬币数</div>
              <div className="font-semibold">{user.stat.coins}</div>
            </div>
          </div>
        </div>
      )}

      {message ? (
        <div className="rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
          {message}
        </div>
      ) : null}
    </div>
  );
}
