PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS anchor (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  uid TEXT NOT NULL UNIQUE,
  nickname TEXT,
  live_status INTEGER DEFAULT 0,
  last_check_time TEXT,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_anchor_uid ON anchor (uid);

CREATE TABLE IF NOT EXISTS login_info (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id INTEGER NOT NULL UNIQUE,
  username TEXT,
  nickname TEXT,
  avatar_url TEXT,
  access_token TEXT,
  refresh_token TEXT,
  cookie_info TEXT,
  login_time TEXT NOT NULL,
  expire_time TEXT,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_login_info_user_id ON login_info (user_id);
CREATE INDEX IF NOT EXISTS idx_login_info_expire_time ON login_info (expire_time);

CREATE TABLE IF NOT EXISTS submission_task (
  task_id TEXT PRIMARY KEY,
  status TEXT NOT NULL,
  priority INTEGER DEFAULT 0,
  title TEXT NOT NULL,
  description TEXT,
  cover_url TEXT,
  cover_local_path TEXT,
  partition_id INTEGER NOT NULL,
  tags TEXT,
  topic_id INTEGER,
  mission_id INTEGER,
  activity_title TEXT,
  video_type TEXT NOT NULL,
  collection_id INTEGER,
  bvid TEXT,
  aid INTEGER,
  remote_state INTEGER,
  reject_reason TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  bilibili_uid INTEGER,
  baidu_uid TEXT,
  segment_prefix TEXT,
  import_mode TEXT DEFAULT 'NON_SEGMENTED',
  baidu_sync_enabled INTEGER DEFAULT 0,
  baidu_sync_path TEXT,
  baidu_sync_filename TEXT
);

CREATE INDEX IF NOT EXISTS idx_submission_task_bilibili_uid ON submission_task (bilibili_uid);

CREATE TABLE IF NOT EXISTS merged_video (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id TEXT NOT NULL,
  sort_order INTEGER DEFAULT 0,
  file_name TEXT,
  video_path TEXT,
  remote_dir TEXT,
  remote_name TEXT,
  baidu_uid TEXT,
  duration INTEGER,
  status INTEGER DEFAULT 0,
  upload_progress REAL DEFAULT 0.0,
  upload_uploaded_bytes INTEGER DEFAULT 0,
  upload_total_bytes INTEGER DEFAULT 0,
  upload_cid INTEGER,
  upload_file_name TEXT,
  upload_session_id TEXT,
  upload_biz_id INTEGER DEFAULT 0,
  upload_endpoint TEXT,
  upload_auth TEXT,
  upload_uri TEXT,
  upload_chunk_size INTEGER DEFAULT 0,
  upload_last_part_index INTEGER DEFAULT 0,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL,
  FOREIGN KEY (task_id) REFERENCES submission_task (task_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS task_output_segment (
  segment_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  merged_id INTEGER,
  part_name TEXT NOT NULL,
  segment_file_path TEXT NOT NULL,
  part_order INTEGER NOT NULL,
  upload_status TEXT NOT NULL,
  cid INTEGER,
  file_name TEXT,
  upload_progress REAL DEFAULT 0.0,
  upload_uploaded_bytes INTEGER DEFAULT 0,
  upload_total_bytes INTEGER DEFAULT 0,
  upload_session_id TEXT,
  upload_biz_id INTEGER DEFAULT 0,
  upload_endpoint TEXT,
  upload_auth TEXT,
  upload_uri TEXT,
  upload_chunk_size INTEGER DEFAULT 0,
  upload_last_part_index INTEGER DEFAULT 0,
  FOREIGN KEY (task_id) REFERENCES submission_task (task_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_task_output_segment_task_id ON task_output_segment (task_id);
CREATE INDEX IF NOT EXISTS idx_task_output_segment_task_merged ON task_output_segment (task_id, merged_id);

CREATE TABLE IF NOT EXISTS task_source_video (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  source_file_path TEXT NOT NULL,
  remote_video_url TEXT,
  remote_bvid TEXT,
  remote_aid INTEGER,
  remote_cid INTEGER,
  remote_part_title TEXT,
  sort_order INTEGER NOT NULL,
  start_time TEXT,
  end_time TEXT,
  FOREIGN KEY (task_id) REFERENCES submission_task (task_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_task_source_video_task_id ON task_source_video (task_id);

CREATE TABLE IF NOT EXISTS merged_source_video (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id TEXT NOT NULL,
  merged_id INTEGER NOT NULL,
  source_id TEXT,
  source_file_path TEXT NOT NULL,
  sort_order INTEGER NOT NULL,
  start_time TEXT,
  end_time TEXT,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL,
  FOREIGN KEY (task_id) REFERENCES submission_task (task_id) ON DELETE CASCADE,
  FOREIGN KEY (merged_id) REFERENCES merged_video (id) ON DELETE CASCADE,
  FOREIGN KEY (source_id) REFERENCES task_source_video (id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_merged_source_video_task_id ON merged_source_video (task_id);
CREATE INDEX IF NOT EXISTS idx_merged_source_video_merged_id ON merged_source_video (merged_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_merged_source_video_unique ON merged_source_video (merged_id, source_file_path, sort_order);

CREATE TABLE IF NOT EXISTS video_clip (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id TEXT NOT NULL,
  file_name TEXT NOT NULL,
  start_time TEXT,
  end_time TEXT,
  clip_path TEXT,
  sequence INTEGER,
  status INTEGER DEFAULT 0,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_video_clip_task_id ON video_clip (task_id);

CREATE TABLE IF NOT EXISTS video_download (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  bvid TEXT,
  aid TEXT,
  title TEXT,
  download_url TEXT,
  local_path TEXT,
  status INTEGER DEFAULT 0,
  progress INTEGER DEFAULT 0,
  progress_total INTEGER DEFAULT 0,
  progress_done INTEGER DEFAULT 0,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL,
  resolution TEXT,
  codec TEXT,
  format TEXT,
  part_title TEXT,
  part_count INTEGER,
  current_part INTEGER,
  cid INTEGER,
  content TEXT,
  source_type TEXT DEFAULT 'BILIBILI'
);

CREATE INDEX IF NOT EXISTS idx_video_download_status ON video_download (status);

CREATE TABLE IF NOT EXISTS video_process_task (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  task_name TEXT,
  status INTEGER DEFAULT 0,
  progress INTEGER DEFAULT 0,
  input_files TEXT,
  output_path TEXT,
  upload_status INTEGER DEFAULT 0,
  bilibili_url TEXT,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_video_process_task_status ON video_process_task (status);

CREATE TABLE IF NOT EXISTS task_relations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  download_task_id INTEGER NOT NULL,
  submission_task_id TEXT NOT NULL,
  relation_type TEXT NOT NULL DEFAULT 'INTEGRATED',
  status TEXT NOT NULL DEFAULT 'ACTIVE',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  workflow_instance_id TEXT,
  workflow_status TEXT DEFAULT 'PENDING_DOWNLOAD',
  workflow_started_at TEXT,
  last_error_message TEXT,
  retry_count INTEGER DEFAULT 0,
  UNIQUE (download_task_id, submission_task_id),
  FOREIGN KEY (download_task_id) REFERENCES video_download (id) ON DELETE CASCADE,
  FOREIGN KEY (submission_task_id) REFERENCES submission_task (task_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_task_relations_download_id ON task_relations (download_task_id);
CREATE INDEX IF NOT EXISTS idx_task_relations_submission_id ON task_relations (submission_task_id);
CREATE INDEX IF NOT EXISTS idx_task_relations_status ON task_relations (status);
CREATE INDEX IF NOT EXISTS idx_task_relations_created_at ON task_relations (created_at);
CREATE INDEX IF NOT EXISTS idx_task_relations_workflow_status ON task_relations (workflow_status);
CREATE INDEX IF NOT EXISTS idx_task_relations_workflow_instance_id ON task_relations (workflow_instance_id);

CREATE TABLE IF NOT EXISTS workflow_instances (
  instance_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  workflow_type TEXT NOT NULL DEFAULT 'VIDEO_SUBMISSION',
  status TEXT NOT NULL DEFAULT 'PENDING',
  current_step TEXT,
  progress REAL DEFAULT 0.0,
  configuration_id INTEGER,
  error_message TEXT,
  started_at TEXT,
  completed_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY (task_id) REFERENCES submission_task (task_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_workflow_instances_task_id ON workflow_instances (task_id);
CREATE INDEX IF NOT EXISTS idx_workflow_instances_status ON workflow_instances (status);
CREATE INDEX IF NOT EXISTS idx_workflow_instances_type ON workflow_instances (workflow_type);
CREATE INDEX IF NOT EXISTS idx_workflow_instances_created_at ON workflow_instances (created_at);
CREATE INDEX IF NOT EXISTS idx_workflow_instances_current_step ON workflow_instances (current_step);

CREATE TABLE IF NOT EXISTS workflow_steps (
  step_id TEXT PRIMARY KEY,
  instance_id TEXT NOT NULL,
  step_name TEXT NOT NULL,
  step_type TEXT NOT NULL,
  step_order INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'PENDING',
  progress REAL DEFAULT 0.0,
  input_data TEXT,
  output_data TEXT,
  error_message TEXT,
  retry_count INTEGER DEFAULT 0,
  max_retries INTEGER DEFAULT 3,
  started_at TEXT,
  completed_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (instance_id, step_order),
  FOREIGN KEY (instance_id) REFERENCES workflow_instances (instance_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_workflow_steps_instance_id ON workflow_steps (instance_id);
CREATE INDEX IF NOT EXISTS idx_workflow_steps_status ON workflow_steps (status);
CREATE INDEX IF NOT EXISTS idx_workflow_steps_type ON workflow_steps (step_type);
CREATE INDEX IF NOT EXISTS idx_workflow_steps_name ON workflow_steps (step_name);
CREATE INDEX IF NOT EXISTS idx_workflow_steps_order ON workflow_steps (step_order);
CREATE INDEX IF NOT EXISTS idx_workflow_steps_created_at ON workflow_steps (created_at);

CREATE TABLE IF NOT EXISTS workflow_configurations (
  config_id INTEGER PRIMARY KEY AUTOINCREMENT,
  config_name TEXT NOT NULL,
  config_type TEXT NOT NULL DEFAULT 'INSTANCE_SPECIFIC',
  user_id INTEGER,
  workflow_type TEXT NOT NULL DEFAULT 'VIDEO_SUBMISSION',
  configuration_data TEXT NOT NULL,
  description TEXT,
  is_active INTEGER NOT NULL DEFAULT 1,
  version INTEGER NOT NULL DEFAULT 1,
  created_by INTEGER,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_workflow_configurations_user_id ON workflow_configurations (user_id);
CREATE INDEX IF NOT EXISTS idx_workflow_configurations_type ON workflow_configurations (config_type);
CREATE INDEX IF NOT EXISTS idx_workflow_configurations_workflow_type ON workflow_configurations (workflow_type);
CREATE INDEX IF NOT EXISTS idx_workflow_configurations_active ON workflow_configurations (is_active);
CREATE INDEX IF NOT EXISTS idx_workflow_configurations_created_at ON workflow_configurations (created_at);
CREATE INDEX IF NOT EXISTS idx_workflow_configurations_name ON workflow_configurations (config_name);

CREATE TABLE IF NOT EXISTS workflow_execution_logs (
  log_id INTEGER PRIMARY KEY AUTOINCREMENT,
  instance_id TEXT NOT NULL,
  step_id TEXT,
  log_level TEXT NOT NULL DEFAULT 'INFO',
  log_message TEXT NOT NULL,
  log_data TEXT,
  source_component TEXT,
  execution_context TEXT,
  created_at TEXT NOT NULL,
  FOREIGN KEY (instance_id) REFERENCES workflow_instances (instance_id) ON DELETE CASCADE,
  FOREIGN KEY (step_id) REFERENCES workflow_steps (step_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_workflow_logs_instance_id ON workflow_execution_logs (instance_id);
CREATE INDEX IF NOT EXISTS idx_workflow_logs_step_id ON workflow_execution_logs (step_id);
CREATE INDEX IF NOT EXISTS idx_workflow_logs_level ON workflow_execution_logs (log_level);
CREATE INDEX IF NOT EXISTS idx_workflow_logs_created_at ON workflow_execution_logs (created_at);
CREATE INDEX IF NOT EXISTS idx_workflow_logs_component ON workflow_execution_logs (source_component);

CREATE TABLE IF NOT EXISTS workflow_performance_metrics (
  metric_id INTEGER PRIMARY KEY AUTOINCREMENT,
  instance_id TEXT NOT NULL,
  step_id TEXT,
  metric_name TEXT NOT NULL,
  metric_value REAL NOT NULL,
  metric_unit TEXT,
  metric_type TEXT NOT NULL,
  measurement_time TEXT NOT NULL,
  additional_data TEXT,
  created_at TEXT NOT NULL,
  FOREIGN KEY (instance_id) REFERENCES workflow_instances (instance_id) ON DELETE CASCADE,
  FOREIGN KEY (step_id) REFERENCES workflow_steps (step_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_workflow_metrics_instance_id ON workflow_performance_metrics (instance_id);
CREATE INDEX IF NOT EXISTS idx_workflow_metrics_step_id ON workflow_performance_metrics (step_id);
CREATE INDEX IF NOT EXISTS idx_workflow_metrics_name ON workflow_performance_metrics (metric_name);
CREATE INDEX IF NOT EXISTS idx_workflow_metrics_type ON workflow_performance_metrics (metric_type);
CREATE INDEX IF NOT EXISTS idx_workflow_metrics_measurement_time ON workflow_performance_metrics (measurement_time);
CREATE INDEX IF NOT EXISTS idx_workflow_metrics_created_at ON workflow_performance_metrics (created_at);

INSERT OR IGNORE INTO workflow_configurations (
  config_id,
  config_name,
  config_type,
  workflow_type,
  configuration_data,
  description,
  version,
  is_active,
  created_at,
  updated_at
) VALUES (
  1,
  'Default Download+Submission Workflow',
  'SYSTEM_DEFAULT',
  'VIDEO_SUBMISSION',
  '{"enableDirectSubmission":true,"enableClipping":true,"enableMerging":true,"enableSegmentation":false,"segmentationConfig":{"segmentDurationSeconds":133,"maxSegments":50,"enableSegmentation":false},"retainOriginalFiles":true,"processingPriority":"NORMAL","maxRetries":3,"timeoutMinutes":30}',
  'Default config - download and submit workflow',
  1,
  1,
  datetime('now'),
  datetime('now')
);

CREATE TABLE IF NOT EXISTS live_settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  file_name_template TEXT NOT NULL,
  record_path TEXT,
  write_metadata INTEGER NOT NULL,
  save_cover INTEGER NOT NULL,
  recording_quality TEXT NOT NULL,
  record_mode INTEGER NOT NULL,
  cutting_mode INTEGER NOT NULL,
  cutting_number INTEGER NOT NULL,
  cutting_by_title INTEGER NOT NULL,
  title_split_min_seconds INTEGER NOT NULL DEFAULT 1800,
  danmaku_transport INTEGER NOT NULL,
  record_danmaku INTEGER NOT NULL,
  record_danmaku_raw INTEGER NOT NULL,
  record_danmaku_superchat INTEGER NOT NULL,
  record_danmaku_gift INTEGER NOT NULL,
  record_danmaku_guard INTEGER NOT NULL,
  stream_retry_ms INTEGER NOT NULL,
  stream_retry_no_qn_sec INTEGER NOT NULL,
  stream_connect_timeout_ms INTEGER NOT NULL,
  stream_read_timeout_ms INTEGER NOT NULL DEFAULT 15000,
  check_interval_sec INTEGER NOT NULL,
  flv_fix_split_on_missing INTEGER NOT NULL,
  flv_fix_adjust_timestamp_jump INTEGER NOT NULL DEFAULT 0,
  flv_fix_split_on_timestamp_jump INTEGER NOT NULL DEFAULT 1,
  flv_fix_disable_on_annexb INTEGER NOT NULL,
  baidu_sync_enabled INTEGER NOT NULL DEFAULT 0,
  baidu_sync_path TEXT,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS live_room_settings (
  room_id TEXT PRIMARY KEY,
  auto_record INTEGER NOT NULL DEFAULT 1,
  baidu_sync_enabled INTEGER NOT NULL DEFAULT 0,
  baidu_sync_path TEXT,
  update_time TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS live_record_task (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  room_id TEXT NOT NULL,
  status TEXT NOT NULL,
  file_path TEXT NOT NULL,
  segment_index INTEGER NOT NULL,
  start_time TEXT NOT NULL,
  end_time TEXT,
  file_size INTEGER DEFAULT 0,
  title TEXT,
  error_message TEXT,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_live_record_task_room_id ON live_record_task (room_id);

CREATE TABLE IF NOT EXISTS app_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

INSERT OR IGNORE INTO app_settings (key, value, updated_at) VALUES
  ('download_threads', '3', datetime('now')),
  ('download_queue_size', '10', datetime('now')),
  ('download_baidu_max_parallel', '3', datetime('now')),
  ('submission_upload_concurrency', '3', datetime('now')),
  ('baidu_sync_enabled', '1', datetime('now')),
  ('baidu_sync_exec_path', '', datetime('now')),
  ('baidu_sync_target_path', '/录播', datetime('now')),
  ('baidu_sync_policy', 'overwrite', datetime('now')),
  ('baidu_sync_retry', '2', datetime('now')),
  ('baidu_sync_concurrency', '3', datetime('now'));

CREATE TABLE IF NOT EXISTS baidu_login_info (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  status TEXT NOT NULL,
  uid TEXT,
  username TEXT,
  login_type TEXT,
  login_time TEXT,
  last_check_time TEXT,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS baidu_login_credential (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  login_type TEXT NOT NULL,
  cookie TEXT,
  bduss TEXT,
  stoken TEXT,
  last_attempt_time TEXT,
  last_attempt_error TEXT,
  create_time TEXT NOT NULL,
  update_time TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS baidu_sync_task (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  source_type TEXT NOT NULL,
  source_id TEXT,
  baidu_uid TEXT,
  source_title TEXT,
  local_path TEXT NOT NULL,
  remote_dir TEXT NOT NULL,
  remote_name TEXT NOT NULL,
  status TEXT NOT NULL,
  progress REAL DEFAULT 0.0,
  error TEXT,
  retry_count INTEGER DEFAULT 0,
  policy TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

INSERT OR IGNORE INTO workflow_configurations (
  config_id,
  config_name,
  config_type,
  workflow_type,
  configuration_data,
  description,
  version,
  is_active,
  created_at,
  updated_at
) VALUES (
  2,
  'Default Submission Task Workflow',
  'SYSTEM_DEFAULT',
  'VIDEO_SUBMISSION',
  '{"enableDirectSubmission":false,"enableClipping":true,"enableMerging":true,"enableSegmentation":true,"segmentationConfig":{"segmentDurationSeconds":133,"maxSegments":50,"enableSegmentation":true},"retainOriginalFiles":true,"processingPriority":"NORMAL","maxRetries":3,"timeoutMinutes":60}',
  'Default config - submission workflow with segmentation',
  1,
  1,
  datetime('now'),
  datetime('now')
);
