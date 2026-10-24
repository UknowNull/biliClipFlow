import { useEffect, useRef, useState } from "react";
import { invokeCommand } from "../../lib/tauri";

const LLM_PRESETS = [
  { key: "ollama", label: "Ollama（本地）", baseUrl: "http://127.0.0.1:11434/v1", local: true },
  { key: "lmstudio", label: "LM Studio（本地）", baseUrl: "http://127.0.0.1:1234/v1", local: true },
  { key: "llamacpp", label: "llama.cpp（本地）", baseUrl: "http://127.0.0.1:8080/v1", local: true },
  { key: "deepseek", label: "DeepSeek", baseUrl: "https://api.deepseek.com/v1" },
  { key: "dashscope", label: "阿里云百炼", baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1" },
  { key: "siliconflow", label: "硅基流动", baseUrl: "https://api.siliconflow.cn/v1" },
  { key: "zhipu", label: "智谱", baseUrl: "https://open.bigmodel.cn/api/paas/v4" },
  { key: "moonshot", label: "Kimi", baseUrl: "https://api.moonshot.cn/v1" },
  { key: "openrouter", label: "OpenRouter", baseUrl: "https://openrouter.ai/api/v1" },
  { key: "custom", label: "自定义", baseUrl: "" },
];

const STRUCTURED_OPTIONS = [
  { value: "auto", label: "自动检测" },
  { value: "json_schema", label: "JSON Schema" },
  { value: "json_object", label: "JSON 对象" },
  { value: "prompt", label: "仅提示词" },
];

const REASONING_OPTIONS = [
  { value: "", label: "不指定" },
  { value: "off", label: "关闭（off）" },
  { value: "low", label: "低（low）" },
  { value: "medium", label: "中（medium）" },
  { value: "high", label: "高（high）" },
];

const MODEL_LABELS = {
  "sensevoice/model.int8.onnx": "SenseVoice 模型（语种/情绪/事件）",
  "sensevoice/tokens.txt": "SenseVoice 词表",
  "paraformer/model.int8.onnx": "Paraformer 模型（中文文本）",
  "paraformer/tokens.txt": "Paraformer 词表",
  "vad/silero_vad.onnx": "VAD 语音分段模型",
};

const inputClass =
  "mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none";
const labelClass = "text-xs uppercase tracking-[0.2em] text-[var(--muted)]";
const cardClass = "rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5";
const ghostButton =
  "rounded-full border border-black/10 bg-white px-4 py-2 text-sm font-semibold text-[var(--ink)] transition hover:border-black/20 disabled:opacity-60";

const formatMb = (bytes) => `${(Number(bytes || 0) / 1024 / 1024).toFixed(1)} MB`;

function Field({ label, hint, children, wide = false }) {
  return (
    <div className={wide ? "lg:col-span-2" : ""}>
      <div className={labelClass}>{label}</div>
      {children}
      {hint ? <div className="mt-1 text-xs text-[var(--muted)]">{hint}</div> : null}
    </div>
  );
}

export default function AutoClipSettings({ visible }) {
  const [settings, setSettings] = useState(null);
  const [asrState, setAsrState] = useState(null);
  const [message, setMessage] = useState("");
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState(null);
  const [models, setModels] = useState([]);
  const [showKey, setShowKey] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const pollRef = useRef(null);

  const load = async () => {
    try {
      const data = await invokeCommand("auto_clip_settings_get");
      setSettings(data.settings);
      setAsrState(data.asr);
    } catch (error) {
      setMessage(error.message);
    }
  };

  useEffect(() => {
    if (visible && !settings) {
      load();
    }
  }, [visible]);

  const refreshAsr = async () => {
    try {
      const data = await invokeCommand("auto_clip_asr_status");
      setAsrState(data);
      return data;
    } catch (error) {
      setMessage(error.message);
      return null;
    }
  };

  useEffect(() => {
    if (!asrState?.download?.running) {
      if (pollRef.current) {
        clearInterval(pollRef.current);
        pollRef.current = null;
      }
      return undefined;
    }
    if (!pollRef.current) {
      pollRef.current = setInterval(refreshAsr, 1000);
    }
    return undefined;
  }, [asrState?.download?.running]);

  useEffect(() => () => pollRef.current && clearInterval(pollRef.current), []);

  if (!settings) {
    return (
      <div className={`${visible ? "block" : "hidden"} ${cardClass}`}>
        <div className="text-sm text-[var(--muted)]">{message || "加载中..."}</div>
      </div>
    );
  }

  const update = (section, key, value) =>
    setSettings((prev) => ({ ...prev, [section]: { ...prev[section], [key]: value } }));
  const clip = settings.clip;
  const llm = settings.llm;
  const asr = settings.asr;

  const handlePreset = (key) => {
    const preset = LLM_PRESETS.find((item) => item.key === key);
    setSettings((prev) => ({
      ...prev,
      llm: {
        ...prev.llm,
        preset: key,
        baseUrl: preset?.baseUrl || prev.llm.baseUrl,
        timeoutSecs: preset?.local ? 180 : 60,
        maxConcurrency: preset?.local ? 1 : 4,
      },
    }));
    setModels([]);
    setTestResult(null);
  };

  const normalized = () => ({
    clip: {
      ...clip,
      candidatesPerHour: Number(clip.candidatesPerHour),
      maxCandidates: Number(clip.maxCandidates),
      minDurationSecs: Number(clip.minDurationSecs),
      maxDurationSecs: Number(clip.maxDurationSecs),
      danmakuLagSecs: Number(clip.danmakuLagSecs),
      minStreamerSpeechPercent: Number(clip.minStreamerSpeechPercent),
      sessionGapMinutes: Number(clip.sessionGapMinutes),
      titleCandidates: Number(clip.titleCandidates),
    },
    llm: {
      ...llm,
      temperature: Number(llm.temperature),
      timeoutSecs: Number(llm.timeoutSecs),
      maxConcurrency: Number(llm.maxConcurrency),
      contextTokens: Number(llm.contextTokens),
    },
    asr: { ...asr, threads: Number(asr.threads) },
  });

  const handleSave = async () => {
    setSaving(true);
    setMessage("");
    try {
      const saved = await invokeCommand("auto_clip_settings_save", { settings: normalized() });
      setSettings(saved);
      await refreshAsr();
      setMessage("自动切片设置已保存");
    } catch (error) {
      setMessage(error.message);
    } finally {
      setSaving(false);
    }
  };

  const handleTest = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      const result = await invokeCommand("auto_clip_llm_test", { llm: normalized().llm });
      setTestResult(result);
      if (result?.ok && llm.structuredOutput === "auto") {
        update("llm", "structuredOutput", result.mode);
      }
    } catch (error) {
      setTestResult({ ok: false, message: error.message });
    } finally {
      setTesting(false);
    }
  };

  const handleListModels = async () => {
    setMessage("");
    try {
      const list = await invokeCommand("auto_clip_llm_models", { llm: normalized().llm });
      setModels(Array.isArray(list) ? list : []);
      if (!list?.length) {
        setMessage("服务没有返回模型列表，请手动填写模型名");
      }
    } catch (error) {
      setMessage(`获取模型列表失败：${error.message}`);
    }
  };

  const handleDownload = async () => {
    setMessage("");
    try {
      await invokeCommand("auto_clip_settings_save", { settings: normalized() });
      await invokeCommand("auto_clip_asr_download");
      await refreshAsr();
    } catch (error) {
      setMessage(error.message);
    }
  };

  const modelFiles = asrState?.models || [];
  const allReady = modelFiles.length > 0 && modelFiles.every((item) => item.ready);
  const download = asrState?.download || {};

  return (
    <div className={`${visible ? "block" : "hidden"} space-y-6`}>
      <div className={cardClass}>
        <div>
          <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">自动切片</p>
          <h2 className="text-2xl font-semibold text-[var(--ink)]">切片参数</h2>
          <p className="mt-2 text-sm text-[var(--muted)]">
            一场直播结束且所有分段转封装完成后自动生成切片，结果在「主播订阅 → 录播 → 切片」中查看。
          </p>
        </div>
        <div className="mt-4 grid gap-3 lg:grid-cols-2">
          <label className="flex items-center gap-2 text-sm text-[var(--ink)] lg:col-span-2">
            <input type="checkbox" checked={clip.enabled} onChange={(e) => update("clip", "enabled", e.target.checked)} />
            直播结束后自动切片（只处理开启之后结束的场次，历史录播可在录播列表中手动分析）
          </label>
          <Field label="每小时候选数" hint="按场次时长计算候选数量">
            <input type="number" step="0.5" className={inputClass} value={clip.candidatesPerHour} onChange={(e) => update("clip", "candidatesPerHour", e.target.value)} />
          </Field>
          <Field label="每场最多候选">
            <input type="number" className={inputClass} value={clip.maxCandidates} onChange={(e) => update("clip", "maxCandidates", e.target.value)} />
          </Field>
          <Field label="切片最短时长（秒）">
            <input type="number" className={inputClass} value={clip.minDurationSecs} onChange={(e) => update("clip", "minDurationSecs", e.target.value)} />
          </Field>
          <Field label="切片最长时长（秒）">
            <input type="number" className={inputClass} value={clip.maxDurationSecs} onChange={(e) => update("clip", "maxDurationSecs", e.target.value)} />
          </Field>
          <Field label="弹幕滞后补偿（秒）" hint="弹幕晚于画面，候选窗口起点前移的秒数">
            <input type="number" className={inputClass} value={clip.danmakuLagSecs} onChange={(e) => update("clip", "danmakuLagSecs", e.target.value)} />
          </Field>
          <Field label="主播解说最低占比（%）" hint="仅对内容类型为 reaction 的主播生效，需要语音识别">
            <input type="number" className={inputClass} value={clip.minStreamerSpeechPercent} onChange={(e) => update("clip", "minStreamerSpeechPercent", e.target.value)} />
          </Field>
          <Field label="场次间隔（分钟）" hint="相邻录播分段间隔不超过该值视为同一场直播">
            <input type="number" className={inputClass} value={clip.sessionGapMinutes} onChange={(e) => update("clip", "sessionGapMinutes", e.target.value)} />
          </Field>
          <Field label="标题候选数">
            <input type="number" className={inputClass} value={clip.titleCandidates} onChange={(e) => update("clip", "titleCandidates", e.target.value)} />
          </Field>
          <label className="flex items-center gap-2 text-sm text-[var(--ink)] lg:col-span-2">
            <input type="checkbox" checked={clip.useLlm} onChange={(e) => update("clip", "useLlm", e.target.checked)} />
            启用 AI 精排（关闭时只按弹幕与语音信号挑选，标题使用模板）
          </label>
        </div>
      </div>

      <div className={cardClass}>
        <div>
          <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">AI</p>
          <h2 className="text-2xl font-semibold text-[var(--ink)]">文本大模型</h2>
          <p className="mt-2 text-sm text-[var(--muted)]">
            使用 OpenAI 兼容接口，可接云端服务或本机的 Ollama、LM Studio、llama.cpp。
          </p>
        </div>
        <div className="mt-4 grid gap-3 lg:grid-cols-2">
          <Field label="服务预设">
            <select className={inputClass} value={llm.preset} onChange={(e) => handlePreset(e.target.value)}>
              {LLM_PRESETS.map((item) => (
                <option key={item.key} value={item.key}>
                  {item.label}
                </option>
              ))}
            </select>
          </Field>
          <Field label="接口地址">
            <input className={inputClass} value={llm.baseUrl} placeholder="https://.../v1" onChange={(e) => update("llm", "baseUrl", e.target.value)} />
          </Field>
          <Field label="API Key" hint="本地服务可留空；已保存的 Key 只显示末四位">
            <div className="mt-2 flex gap-2">
              <input
                type={showKey ? "text" : "password"}
                className={inputClass.replace("mt-2 ", "")}
                value={llm.apiKey}
                onChange={(e) => update("llm", "apiKey", e.target.value)}
              />
              <button type="button" className={ghostButton} onClick={() => setShowKey((v) => !v)}>
                {showKey ? "隐藏" : "显示"}
              </button>
            </div>
          </Field>
          <Field label="模型名">
            <div className="mt-2 flex gap-2">
              <input
                list="auto-clip-models"
                className={inputClass.replace("mt-2 ", "")}
                value={llm.model}
                onChange={(e) => update("llm", "model", e.target.value)}
              />
              <datalist id="auto-clip-models">
                {models.map((m) => (
                  <option key={m} value={m} />
                ))}
              </datalist>
              <button type="button" className={ghostButton} onClick={handleListModels}>
                获取列表
              </button>
            </div>
          </Field>
          <Field label="结构化输出" hint="自动检测会在测试连接时选出服务支持的方式">
            <select className={inputClass} value={llm.structuredOutput} onChange={(e) => update("llm", "structuredOutput", e.target.value)}>
              {STRUCTURED_OPTIONS.map((item) => (
                <option key={item.value} value={item.value}>
                  {item.label}
                </option>
              ))}
            </select>
          </Field>
          <Field label="思考档位" hint="推理模型的思考过程计入输出 token，关闭可大幅节省；不指定时由服务端决定">
            <select className={inputClass} value={llm.reasoningEffort || ""} onChange={(e) => update("llm", "reasoningEffort", e.target.value)}>
              {REASONING_OPTIONS.map((item) => (
                <option key={item.value} value={item.value}>
                  {item.label}
                </option>
              ))}
            </select>
          </Field>
          <div className="flex items-end">
            <button type="button" className="text-xs text-blue-600 hover:underline" onClick={() => setShowAdvanced((v) => !v)}>
              {showAdvanced ? "收起高级项" : "展开高级项"}
            </button>
          </div>
          {showAdvanced ? (
            <>
              <Field label="温度">
                <input type="number" step="0.1" className={inputClass} value={llm.temperature} onChange={(e) => update("llm", "temperature", e.target.value)} />
              </Field>
              <Field label="超时（秒）">
                <input type="number" className={inputClass} value={llm.timeoutSecs} onChange={(e) => update("llm", "timeoutSecs", e.target.value)} />
              </Field>
              <Field label="最大并发">
                <input type="number" className={inputClass} value={llm.maxConcurrency} onChange={(e) => update("llm", "maxConcurrency", e.target.value)} />
              </Field>
              <Field label="上下文长度">
                <input type="number" className={inputClass} value={llm.contextTokens} onChange={(e) => update("llm", "contextTokens", e.target.value)} />
              </Field>
            </>
          ) : null}
        </div>
        <div className="mt-4 flex flex-wrap items-center gap-3">
          <button type="button" className={ghostButton} onClick={handleTest} disabled={testing}>
            {testing ? "测试中..." : "测试连接"}
          </button>
          {testResult ? (
            <span className={`text-sm ${testResult.ok ? "text-emerald-700" : "text-red-600"}`}>
              {testResult.ok
                ? `连接成功：${STRUCTURED_OPTIONS.find((o) => o.value === testResult.mode)?.label || testResult.mode}，耗时 ${testResult.latencyMs} 毫秒`
                : `连接失败：${testResult.message}`}
            </span>
          ) : null}
        </div>
      </div>

      <div className={cardClass}>
        <div>
          <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">AI</p>
          <h2 className="text-2xl font-semibold text-[var(--ink)]">语音识别</h2>
          <p className="mt-2 text-sm text-[var(--muted)]">
            本地模式使用 SenseVoice 识别语种、情绪与笑声，Paraformer 识别主播中文，模型约 480MB，首次使用需下载。
          </p>
        </div>
        <div className="mt-4 grid gap-3 lg:grid-cols-2">
          <Field label="识别方式">
            <div className="mt-2 flex gap-4 text-sm text-[var(--ink)]">
              <label className="flex items-center gap-2">
                <input type="radio" checked={asr.mode === "local"} onChange={() => update("asr", "mode", "local")} />
                本地模型
              </label>
              <label className="flex items-center gap-2 text-[var(--muted)]" title="后续版本支持">
                <input type="radio" disabled checked={asr.mode === "api"} />
                API（即将支持）
              </label>
            </div>
          </Field>
          <Field label="线程数">
            <input type="number" className={inputClass} value={asr.threads} onChange={(e) => update("asr", "threads", e.target.value)} />
          </Field>
          <Field label="模型目录" hint={`留空使用默认目录：${asrState?.defaultModelDir || ""}`} wide>
            <input className={inputClass} value={asr.localModelDir} placeholder={asrState?.defaultModelDir || ""} onChange={(e) => update("asr", "localModelDir", e.target.value)} />
          </Field>
          <Field label="下载源">
            <select className={inputClass} value={asr.downloadSource} onChange={(e) => update("asr", "downloadSource", e.target.value)}>
              <option value="hf_mirror">hf-mirror（国内镜像）</option>
              <option value="huggingface">Hugging Face</option>
            </select>
          </Field>
        </div>
        <div className="mt-4 space-y-2">
          {modelFiles.map((item) => (
            <div key={item.path} className="flex items-center justify-between rounded-lg bg-white/70 px-3 py-2 text-sm">
              <span className="text-[var(--ink)]">{MODEL_LABELS[item.path] || item.path}</span>
              <span className={item.ready ? "text-emerald-700" : "text-[var(--muted)]"}>
                {item.ready ? `已就绪 ${formatMb(item.size)}` : `未下载（${formatMb(item.expectedSize)}）`}
              </span>
            </div>
          ))}
        </div>
        <div className="mt-4 flex flex-wrap items-center gap-3">
          <button type="button" className={ghostButton} onClick={handleDownload} disabled={download.running || allReady}>
            {allReady ? "模型已就绪" : download.running ? "下载中..." : "下载模型"}
          </button>
          {download.running ? (
            <span className="text-sm text-[var(--muted)]">
              {MODEL_LABELS[download.currentFile] || download.currentFile}：{formatMb(download.downloaded)} / {formatMb(download.total)}
            </span>
          ) : download.error ? (
            <span className="text-sm text-red-600">{download.error}</span>
          ) : null}
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-3">
        <button
          className="rounded-full bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white shadow-sm transition hover:brightness-110 disabled:opacity-60"
          onClick={handleSave}
          disabled={saving}
        >
          {saving ? "保存中..." : "保存"}
        </button>
        <button className={ghostButton} onClick={load}>
          刷新
        </button>
        {message ? <span className="text-sm text-amber-700">{message}</span> : null}
      </div>
    </div>
  );
}
