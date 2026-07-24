import { useCallback, useEffect, useState } from "react";
import { invokeCommand } from "../../lib/tauri";
import VideoMaskTool from "./VideoMaskTool";

const ACTIVE_PROJECT_STORAGE_KEY = "biliClipFlow.videoMask.activeProjectId";

const readActiveProjectId = () => {
  try {
    return sessionStorage.getItem(ACTIVE_PROJECT_STORAGE_KEY) || "";
  } catch {
    return "";
  }
};

const rememberActiveProjectId = (projectId) => {
  try {
    if (projectId) {
      sessionStorage.setItem(ACTIVE_PROJECT_STORAGE_KEY, projectId);
    } else {
      sessionStorage.removeItem(ACTIVE_PROJECT_STORAGE_KEY);
    }
  } catch {
    // 会话存储不可用时仍允许使用项目功能。
  }
};

const formatProjectTime = (value) => {
  const date = new Date(value || "");
  return Number.isNaN(date.getTime()) ? "-" : date.toLocaleString();
};

const formatDuration = (seconds) => {
  const value = Math.max(0, Math.round(Number(seconds) || 0));
  const hours = Math.floor(value / 3600);
  const minutes = Math.floor((value % 3600) / 60);
  const rest = value % 60;
  return [hours, minutes, rest].map((item) => String(item).padStart(2, "0")).join(":");
};

const projectSourceName = (path) => {
  const normalized = String(path || "").replace(/\\/g, "/");
  return normalized.split("/").filter(Boolean).pop() || "尚未导入视频";
};

export default function VideoMaskProjectTool() {
  const [projects, setProjects] = useState([]);
  const [activeProject, setActiveProject] = useState(null);
  const [projectName, setProjectName] = useState("");
  const [renamingId, setRenamingId] = useState("");
  const [renamingName, setRenamingName] = useState("");
  const [loading, setLoading] = useState(true);
  const [operatingId, setOperatingId] = useState("");
  const [message, setMessage] = useState("");

  const loadProjects = useCallback(async () => {
    const data = await invokeCommand("toolbox_video_mask_project_list");
    setProjects(Array.isArray(data) ? data : []);
  }, []);

  const openProject = useCallback(async (projectId) => {
    setMessage("");
    setOperatingId(`open-${projectId}`);
    try {
      const project = await invokeCommand("toolbox_video_mask_project_detail", {
        payload: { projectId },
      });
      setActiveProject(project);
      rememberActiveProjectId(project.id);
    } catch (error) {
      rememberActiveProjectId("");
      setMessage(error?.message || "打开项目失败");
    } finally {
      setOperatingId("");
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    const initialize = async () => {
      setLoading(true);
      try {
        const data = await invokeCommand("toolbox_video_mask_project_list");
        if (cancelled) {
          return;
        }
        setProjects(Array.isArray(data) ? data : []);
        const activeProjectId = readActiveProjectId();
        if (activeProjectId) {
          const project = await invokeCommand("toolbox_video_mask_project_detail", {
            payload: { projectId: activeProjectId },
          });
          if (!cancelled) {
            setActiveProject(project);
          }
        }
      } catch (error) {
        if (!cancelled) {
          rememberActiveProjectId("");
          setMessage(error?.message || "读取视频遮罩项目失败");
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    };
    void initialize();
    return () => {
      cancelled = true;
    };
  }, []);

  const handleCreate = async () => {
    setMessage("");
    setOperatingId("create");
    try {
      const project = await invokeCommand("toolbox_video_mask_project_create", {
        payload: { name: projectName.trim() || null },
      });
      setProjectName("");
      setActiveProject(project);
      rememberActiveProjectId(project.id);
    } catch (error) {
      setMessage(error?.message || "创建项目失败");
    } finally {
      setOperatingId("");
    }
  };

  const handleRename = async (projectId) => {
    const name = renamingName.trim();
    if (!name) {
      setMessage("项目名称不能为空");
      return;
    }
    setOperatingId(`rename-${projectId}`);
    try {
      await invokeCommand("toolbox_video_mask_project_rename", {
        payload: { projectId, name },
      });
      setRenamingId("");
      setRenamingName("");
      await loadProjects();
    } catch (error) {
      setMessage(error?.message || "重命名项目失败");
    } finally {
      setOperatingId("");
    }
  };

  const handleDelete = async (project) => {
    if (!window.confirm(`确定删除项目“${project.name}”吗？项目内复制的图片资源会一并删除，源视频和导出视频不受影响。`)) {
      return;
    }
    setOperatingId(`delete-${project.id}`);
    setMessage("");
    try {
      await invokeCommand("toolbox_video_mask_project_delete", {
        payload: { projectId: project.id },
      });
      await loadProjects();
    } catch (error) {
      setMessage(error?.message || "删除项目失败");
    } finally {
      setOperatingId("");
    }
  };

  const handleProjectSaved = useCallback((saved) => {
    setActiveProject((prev) => (prev ? { ...prev, revision: saved.revision, updatedAt: saved.updatedAt } : prev));
  }, []);

  const handleBack = useCallback(async () => {
    rememberActiveProjectId("");
    setActiveProject(null);
    setLoading(true);
    try {
      await loadProjects();
    } catch (error) {
      setMessage(error?.message || "刷新项目列表失败");
    } finally {
      setLoading(false);
    }
  }, [loadProjects]);

  if (activeProject) {
    return (
      <VideoMaskTool
        key={activeProject.id}
        project={activeProject}
        onBack={handleBack}
        onProjectSaved={handleProjectSaved}
      />
    );
  }

  return (
    <div className="space-y-4">
      <div className="panel p-4 space-y-4">
        <div>
          <div className="text-lg font-semibold">视频遮罩项目</div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <input
            className="min-w-[240px] flex-1"
            value={projectName}
            onChange={(event) => setProjectName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !operatingId) {
                void handleCreate();
              }
            }}
            placeholder="项目名称"
            maxLength={80}
          />
          <button className="h-9 rounded-lg px-4" onClick={handleCreate} disabled={Boolean(operatingId)}>
            {operatingId === "create" ? "创建中..." : "新建项目"}
          </button>
        </div>
        {message ? <div className="text-xs text-red-500">{message}</div> : null}
      </div>

      <div className="panel p-4 space-y-3">
        <div className="flex items-center justify-between gap-3">
          <div className="text-sm font-semibold">项目列表</div>
          <button className="h-8 rounded-lg px-3" onClick={() => void loadProjects()} disabled={loading || Boolean(operatingId)}>
            刷新
          </button>
        </div>
        {loading ? (
          <div className="py-8 text-center text-sm text-[var(--desc-color)]">正在读取项目...</div>
        ) : projects.length > 0 ? (
          <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
            {projects.map((project) => (
              <div key={project.id} className="rounded-lg border border-[var(--split-color)] bg-[var(--solid-button-color)] p-4">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0 flex-1">
                    {renamingId === project.id ? (
                      <input
                        className="w-full"
                        value={renamingName}
                        onChange={(event) => setRenamingName(event.target.value)}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") {
                            void handleRename(project.id);
                          }
                          if (event.key === "Escape") {
                            setRenamingId("");
                          }
                        }}
                        autoFocus
                        maxLength={80}
                      />
                    ) : (
                      <div className="truncate font-semibold" title={project.name}>{project.name}</div>
                    )}
                    <div className="mt-2 truncate text-xs text-[var(--desc-color)]" title={project.sourcePath || ""}>
                      {projectSourceName(project.sourcePath)} · {formatDuration(project.duration)}
                    </div>
                    <div className="mt-1 text-xs text-[var(--desc-color)]">最近编辑 {formatProjectTime(project.updatedAt)}</div>
                    {project.sourcePath && !project.sourceExists ? (
                      <div className="mt-2 text-xs text-red-500">源视频已移动或删除，打开后可重新关联</div>
                    ) : null}
                  </div>
                </div>
                <div className="mt-4 flex flex-wrap items-center gap-2">
                  <button className="h-8 rounded-lg px-3" onClick={() => void openProject(project.id)} disabled={Boolean(operatingId)}>
                    {operatingId === `open-${project.id}` ? "打开中..." : "打开"}
                  </button>
                  {renamingId === project.id ? (
                    <>
                      <button className="h-8 rounded-lg px-3" onClick={() => void handleRename(project.id)} disabled={Boolean(operatingId)}>保存名称</button>
                      <button className="h-8 rounded-lg border border-[var(--split-color)] bg-[var(--card-bg)] px-3" onClick={() => setRenamingId("")} disabled={Boolean(operatingId)}>取消</button>
                    </>
                  ) : (
                    <button
                      className="h-8 rounded-lg border border-[var(--split-color)] bg-[var(--card-bg)] px-3"
                      onClick={() => {
                        setRenamingId(project.id);
                        setRenamingName(project.name);
                      }}
                      disabled={Boolean(operatingId)}
                    >
                      重命名
                    </button>
                  )}
                  <button className="h-8 rounded-lg bg-red-500 px-3 text-white hover:bg-red-600 disabled:opacity-60" onClick={() => void handleDelete(project)} disabled={Boolean(operatingId)}>
                    {operatingId === `delete-${project.id}` ? "删除中..." : "删除"}
                  </button>
                </div>
              </div>
            ))}
          </div>
        ) : (
          <div className="rounded-lg border border-dashed border-[var(--split-color)] py-10 text-center text-sm text-[var(--desc-color)]">
            暂无项目，请先新建项目。
          </div>
        )}
      </div>
    </div>
  );
}
