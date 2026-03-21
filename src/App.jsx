import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import AnchorSection from "./sections/AnchorSection";
import DownloadSection from "./sections/DownloadSection";
import SubmissionSection from "./sections/SubmissionSection";
import SubmissionSyncSection from "./sections/SubmissionSyncSection";
import SettingsSection from "./sections/SettingsSection";
import LoginSection from "./sections/LoginSection";
import ToolboxSection from "./sections/ToolboxSection";
import AboutSection from "./sections/AboutSection";
import { invokeCommand } from "./lib/tauri";
import { showErrorDialog } from "./lib/dialog";

const sections = [
  { id: "anchor", label: "主播订阅", short: "订" },
  { id: "download", label: "视频下载", short: "下" },
  { id: "submission", label: "视频投稿", short: "投" },
  { id: "submission_sync", label: "视频同步", short: "同" },
  {
    id: "toolbox",
    label: "工具箱",
    short: "工",
    children: [{ id: "toolbox.remux", label: "格式转码" }],
  },
  { id: "settings", label: "设置", short: "设" },
  { id: "about", label: "关于", short: "关" },
];

const sectionLabels = {
  auth: "登录",
  anchor: "主播订阅",
  download: "视频下载",
  submission: "视频投稿",
  submission_sync: "视频同步",
  toolbox: "工具箱",
  "toolbox.remux": "格式转码",
  settings: "设置",
  about: "关于",
};

function App() {
  const [active, setActive] = useState("download");
  const [expandedMenus, setExpandedMenus] = useState({ toolbox: false });
  const [authStatus, setAuthStatus] = useState({ loggedIn: false });
  const [avatarPreviews, setAvatarPreviews] = useState({});
  const [accountBindings, setAccountBindings] = useState({});
  const [baiduStatus, setBaiduStatus] = useState({ status: "LOGGED_OUT" });
  const [biliAddRequestKey, setBiliAddRequestKey] = useState(0);
  const [avatarMenu, setAvatarMenu] = useState({
    open: false,
    x: 0,
    y: 0,
    userId: "",
  });
  const avatarClickTimerRef = useRef(null);

  const activeSection = useMemo(() => active.split(".")[0], [active]);

  const activeLabel = useMemo(() => {
    if (active.includes(".")) {
      const parent = active.split(".")[0];
      const parentLabel = sectionLabels[parent] || "";
      const childLabel = sectionLabels[active] || "";
      return parentLabel && childLabel ? `${parentLabel} / ${childLabel}` : parentLabel || childLabel;
    }
    return sectionLabels[active] || "";
  }, [active]);

  const bilibiliAccounts = useMemo(
    () => (Array.isArray(authStatus?.accounts) ? authStatus.accounts : []),
    [authStatus],
  );
  const activeBilibiliUid = useMemo(
    () => String(authStatus?.activeAccount?.userId || "").trim(),
    [authStatus],
  );
  const primaryBilibiliUid = useMemo(
    () => String(authStatus?.primaryAccount?.userId || authStatus?.primaryAccountUserId || "").trim(),
    [authStatus],
  );

  const refreshAuthStatus = useCallback(async () => {
    try {
      const data = await invokeCommand("auth_status");
      setAuthStatus(data || { loggedIn: false });
    } catch (error) {
      setAuthStatus((prev) => prev || { loggedIn: false });
    }
  }, []);

  const refreshBaiduStatus = useCallback(async () => {
    try {
      const data = await invokeCommand("baidu_sync_status");
      setBaiduStatus(data || { status: "LOGGED_OUT" });
    } catch (error) {
      setBaiduStatus((prev) => prev || { status: "LOGGED_OUT" });
    }
  }, []);

  const refreshAccountBindings = useCallback(async () => {
    try {
      const data = await invokeCommand("account_binding_list");
      const nextBindings = {};
      (Array.isArray(data) ? data : []).forEach((item) => {
        const bilibiliUid = String(item?.bilibiliUid || item?.bilibili_uid || "").trim();
        const baiduUid = String(item?.baiduUid || item?.baidu_uid || "").trim();
        if (bilibiliUid && baiduUid) {
          nextBindings[bilibiliUid] = baiduUid;
        }
      });
      setAccountBindings(nextBindings);
    } catch (_) {
      setAccountBindings({});
    }
  }, []);

  useEffect(() => {
    refreshAuthStatus();
    refreshBaiduStatus();
    refreshAccountBindings();
  }, [refreshAuthStatus, refreshBaiduStatus, refreshAccountBindings]);

  useEffect(() => {
    const parent = active.split(".")[0];
    const hasChildren = sections.some((item) => item.id === parent && item.children?.length);
    if (!hasChildren) {
      return;
    }
    setExpandedMenus((prev) => {
      if (prev[parent]) {
        return prev;
      }
      return { ...prev, [parent]: true };
    });
  }, [active]);

  useEffect(() => {
    const loadAvatars = async () => {
      if (!authStatus?.loggedIn || bilibiliAccounts.length === 0) {
        setAvatarPreviews({});
        return;
      }
      const nextPreviews = {};
      for (const account of bilibiliAccounts) {
        const userId = String(account?.userId || "").trim();
        const avatarUrl = String(account?.avatarUrl || "").trim();
        if (!userId) {
          continue;
        }
        if (!avatarUrl) {
          nextPreviews[userId] = "";
          continue;
        }
        try {
          const data = await invokeCommand("video_proxy_image", { url: avatarUrl });
          nextPreviews[userId] = data || "";
        } catch (_) {
          nextPreviews[userId] = "";
        }
      }
      setAvatarPreviews(nextPreviews);
    };
    loadAvatars();
  }, [authStatus?.loggedIn, bilibiliAccounts]);

  useEffect(
    () => () => {
      if (avatarClickTimerRef.current) {
        clearTimeout(avatarClickTimerRef.current);
      }
    },
    [],
  );

  useEffect(() => {
    if (!avatarMenu.open) {
      return undefined;
    }
    const closeMenu = () => {
      setAvatarMenu((prev) => (prev.open ? { ...prev, open: false } : prev));
    };
    window.addEventListener("click", closeMenu);
    window.addEventListener("blur", closeMenu);
    window.addEventListener("contextmenu", closeMenu);
    return () => {
      window.removeEventListener("click", closeMenu);
      window.removeEventListener("blur", closeMenu);
      window.removeEventListener("contextmenu", closeMenu);
    };
  }, [avatarMenu.open]);

  const switchAvatarAccount = useCallback(
    async (userId, openDetail = false) => {
      const normalized = String(userId || "").trim();
      if (!normalized) {
        if (openDetail) {
          setActive("auth");
        }
        return;
      }
      try {
        if (normalized !== activeBilibiliUid) {
          const data = await invokeCommand("auth_account_switch", {
            userId: Number(normalized),
          });
          setAuthStatus(data || { loggedIn: false });
          await refreshBaiduStatus();
        }
        if (openDetail) {
          setActive("auth");
        }
      } catch (error) {
        await showErrorDialog(error, openDetail ? "打开账号详情失败" : "切换账号失败");
      }
    },
    [activeBilibiliUid, refreshBaiduStatus],
  );

  const setPrimaryAvatarAccount = useCallback(
    async (userId) => {
      const normalized = String(userId || "").trim();
      if (!normalized || normalized === primaryBilibiliUid) {
        setAvatarMenu((prev) => ({ ...prev, open: false }));
        return;
      }
      try {
        const data = await invokeCommand("auth_account_set_primary", {
          userId: Number(normalized),
        });
        setAuthStatus(data || { loggedIn: false });
      } catch (error) {
        await showErrorDialog(error, "设置主账号失败");
      } finally {
        setAvatarMenu((prev) => ({ ...prev, open: false }));
      }
    },
    [primaryBilibiliUid],
  );

  const openAvatarContextMenu = useCallback((event, userId) => {
    event.preventDefault();
    event.stopPropagation();
    setAvatarMenu({
      open: true,
      x: event.clientX,
      y: event.clientY,
      userId: String(userId || "").trim(),
    });
  }, []);

  const renderSection = () => {
    switch (activeSection) {
      case "auth":
        return (
          <LoginSection
            authStatus={authStatus}
            onAuthChange={setAuthStatus}
            baiduStatus={baiduStatus}
            onBaiduChange={setBaiduStatus}
            onRefreshBaidu={refreshBaiduStatus}
            onBindingChange={refreshAccountBindings}
            biliAddRequestKey={biliAddRequestKey}
          />
        );
      case "anchor":
        return <AnchorSection />;
      case "download":
        return (
          <DownloadSection
            activeBilibiliUid={activeBilibiliUid}
            onAuthChange={setAuthStatus}
            onRefreshBaiduStatus={refreshBaiduStatus}
          />
        );
      case "submission":
        return (
          <SubmissionSection
            activeBilibiliUid={activeBilibiliUid}
            onAuthChange={setAuthStatus}
            onRefreshBaiduStatus={refreshBaiduStatus}
          />
        );
      case "submission_sync":
        return <SubmissionSyncSection />;
      case "toolbox":
        return <ToolboxSection />;
      case "settings":
        return <SettingsSection />;
      case "about":
        return <AboutSection />;
      default:
        return null;
    }
  };

  return (
    <div className="app-shell">
      <aside className="sidebar">
        {sections.map((item) => {
          const hasChildren = Boolean(item.children?.length);
          const isParentActive = activeSection === item.id;
          if (!hasChildren) {
            return (
              <button
                key={item.id}
                className={activeSection === item.id ? "active" : ""}
                onClick={() => setActive(item.id)}
                title={item.label}
              >
                <span className="menu-label">{item.label}</span>
              </button>
            );
          }
          const expanded = Boolean(expandedMenus[item.id]);
          return (
            <div
              key={item.id}
              className={expanded ? "menu-group expanded" : "menu-group"}
            >
              <button
                className={isParentActive ? "active" : ""}
                onClick={() =>
                  setExpandedMenus((prev) => ({
                    ...prev,
                    [item.id]: !prev[item.id],
                  }))
                }
                title={item.label}
              >
                <span className="menu-label">{item.label}</span>
                <span className="menu-caret" />
              </button>
              {expanded ? (
                <div className="submenu">
                  {item.children.map((child) => (
                    <button
                      key={child.id}
                      className={active === child.id ? "active submenu-item" : "submenu-item"}
                      onClick={() => setActive(child.id)}
                      title={child.label}
                    >
                      <span className="menu-label">{child.label}</span>
                    </button>
                  ))}
                </div>
              ) : null}
            </div>
          );
        })}
      </aside>
      <div id="main" className="main-shell">
        <div className="title-bar" data-tauri-drag-region>
          <div />
          <div className="avatar-actions" data-tauri-drag-region="false">
            <button
              className="avatar-add-btn"
              onClick={() => {
                setActive("auth");
                setBiliAddRequestKey((prev) => prev + 1);
              }}
              title="新增Bilibili账号"
            >
              +
            </button>
            <div className="avatar-list">
              {bilibiliAccounts.length > 0 ? (
                bilibiliAccounts.map((account) => {
                  const userId = String(account?.userId || "").trim();
                  const avatarPreview = avatarPreviews[userId] || "";
                  const isActiveAccount = Boolean(account?.isActive);
                  const isPrimaryAccount = Boolean(account?.isPrimary);
                  const hasBaiduBinding = Boolean(accountBindings[userId]);
                  return (
                    <button
                      key={userId}
                      className={`avatar-btn ${active === "auth" && isActiveAccount ? "active" : ""} ${isActiveAccount ? "current-account" : ""} ${isPrimaryAccount ? "primary-account" : ""}`}
                      data-tauri-drag-region="false"
                      onClick={() => {
                        if (avatarClickTimerRef.current) {
                          clearTimeout(avatarClickTimerRef.current);
                        }
                        avatarClickTimerRef.current = setTimeout(() => {
                          switchAvatarAccount(userId, false);
                          avatarClickTimerRef.current = null;
                        }, 220);
                      }}
                      onDoubleClick={() => {
                        if (avatarClickTimerRef.current) {
                          clearTimeout(avatarClickTimerRef.current);
                          avatarClickTimerRef.current = null;
                        }
                        switchAvatarAccount(userId, true);
                      }}
                      onMouseDown={(event) => {
                        if (event.button !== 2) {
                          return;
                        }
                        openAvatarContextMenu(event, userId);
                      }}
                      onContextMenu={(event) => {
                        openAvatarContextMenu(event, userId);
                      }}
                      title={account?.nickname || account?.username || "登录"}
                    >
                      {avatarPreview ? (
                        <img
                          src={avatarPreview}
                          alt="用户头像"
                          onError={() => {
                            setAvatarPreviews((prev) => ({ ...prev, [userId]: "" }));
                          }}
                        />
                      ) : (
                        <span className="avatar-fallback" />
                      )}
                      {isPrimaryAccount ? (
                        <span className="avatar-badge avatar-primary-badge" title="主账号">
                          主
                        </span>
                      ) : null}
                      {hasBaiduBinding ? (
                        <span
                          className="avatar-badge avatar-baidu-badge"
                          title={`已绑定网盘账号${accountBindings[userId] ? `：${accountBindings[userId]}` : ""}`}
                        >
                          网
                        </span>
                      ) : null}
                    </button>
                  );
                })
              ) : (
                <button
                  className={`avatar-btn ${active === "auth" ? "active" : ""}`}
                  onClick={() => setActive("auth")}
                  title="登录"
                >
                  <span className="avatar-fallback" />
                </button>
              )}
            </div>
            {avatarMenu.open ? (
              <div
                className="avatar-context-menu"
                data-tauri-drag-region="false"
                style={{ left: avatarMenu.x, top: avatarMenu.y }}
              >
                <button
                  className="avatar-context-item"
                  disabled={avatarMenu.userId === primaryBilibiliUid}
                  onClick={() => setPrimaryAvatarAccount(avatarMenu.userId)}
                >
                  {avatarMenu.userId === primaryBilibiliUid ? "当前已是主账号" : "设为主账号"}
                </button>
              </div>
            ) : null}
          </div>
        </div>
        <div className="content-wrap">
          <div className="page">
            <div className="page-scroll">{renderSection()}</div>
          </div>
        </div>
      </div>
    </div>
  );
}

export default App;
