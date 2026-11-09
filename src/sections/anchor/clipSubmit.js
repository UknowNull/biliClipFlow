import { invokeCommand } from "../../lib/tauri";
import { renderCoverDataUrl } from "./cover";

// 投稿标题：投稿设置里的标题（如【主播名】）加切片标题，切片标题已带前缀时不重复
export const joinClipTitle = (prefix, clipTitle) => (prefix && !clipTitle.startsWith(prefix) ? `${prefix}${clipTitle}` : clipTitle);

export const clipFormError = (form, tags) => {
  if (!form.title.trim()) return "请输入投稿标题";
  if (form.title.length > 80) return "投稿标题不能超过 80 个字符";
  if (!form.partitionId) return "请选择B站分区";
  if (!form.videoType) return "请选择视频类型";
  if (form.description && form.description.length > 2000) return "视频描述不能超过 2000 个字符";
  if (!tags.length) return "请填写至少一个投稿标签";
  return "";
};

// 创建一条切片投稿任务。submission_create 默认创建后立即执行，不能再调用 submission_execute，否则同一任务会跑两遍；
// 带字幕时，字幕烧录是投稿工作流的第一步（按切片当前起止从录播重新剪辑并压入字幕）
export async function createClipSubmission(record, form, tags) {
  const clipPath = record?.filePath;
  if (!clipPath) throw new Error("切片文件路径为空，无法投稿");
  const subtitle = record.subtitle;
  // 投稿要求源视频写明起止：按现有切片时长填写（向下取整再加 1 秒，ffmpeg 读到文件尾即止），烧录字幕后工作流会换成新文件的时长
  const durationSeconds = Number(await invokeCommand("video_duration", { path: clipPath })) || 0;
  if (!durationSeconds) throw new Error("读取切片时长失败，无法投稿");
  const end = durationSeconds + 1;
  const pad = (n) => String(n).padStart(2, "0");
  // 有封面参数时按参数渲染封面并上传，拿到封面地址随稿件提交
  const coverUrl = record.coverSpec?.frame?.path
    ? await invokeCommand("submission_upload_cover", { request: { imageDataUrl: await renderCoverDataUrl(record.coverSpec) } })
    : null;
  await invokeCommand("submission_create", {
    request: {
      task: {
        title: form.title.trim(),
        coverUrl,
        description: form.description?.trim() || "",
        partitionId: Number(form.partitionId),
        collectionId: form.collectionId ? Number(form.collectionId) : null,
        tags: tags.join(","),
        topicId: form.activityTopicId ? Number(form.activityTopicId) : null,
        missionId: form.activityMissionId ? Number(form.activityMissionId) : null,
        activityTitle: form.activityTitle || null,
        videoType: form.videoType,
        segmentPrefix: null,
        priority: false,
        baiduSyncEnabled: false,
        baiduSyncPath: null,
        baiduSyncFilename: null,
        sourceType: "CLIP",
      },
      sourceVideos: [{ sourceFilePath: clipPath, sortOrder: 1, startTime: "00:00:00", endTime: `${pad(Math.floor(end / 3600))}:${pad(Math.floor((end % 3600) / 60))}:${pad(end % 60)}` }],
      workflowConfig: {
        enableSegmentation: false,
        segmentationConfig: { enabled: false, segmentDurationSeconds: 133, preserveOriginal: true },
        // 总是带上切片编号：投稿时要按最新边界从录播重剪，字幕有就一起烧进去
        subtitleBurn: { clipId: record.id, cues: subtitle?.cues || [], style: subtitle?.style || {} },
      },
    },
  });
}
