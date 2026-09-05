use super::models::{AccountSummary, CredentialSummary, YouTubeSnapshot};
use crate::AppError;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PersistedState {
    version: u32,
    credential: CredentialSummary,
    channels: Vec<AccountSummary>,
    active_channel_id: Option<String>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            credential: CredentialSummary::default(),
            channels: Vec::new(),
            active_channel_id: None,
        }
    }
}

pub struct YouTubeStateStore {
    path: PathBuf,
    state: Mutex<PersistedState>,
}

impl YouTubeStateStore {
    pub fn load(app_config_dir: &Path) -> Result<Self, AppError> {
        let path = app_config_dir.join("youtube/youtube-state.json");
        let state = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<PersistedState>(&bytes)
                .ok()
                .filter(|value| value.version == STATE_VERSION)
                .unwrap_or_default(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => PersistedState::default(),
            Err(error) => return Err(state_error(error)),
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    pub fn snapshot(&self) -> YouTubeSnapshot {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        YouTubeSnapshot {
            credential: state.credential.clone(),
            channels: state.channels.clone(),
            active_channel_id: state.active_channel_id.clone(),
            jobs: Vec::new(),
        }
    }

    pub fn set_credential(&self, credential: CredentialSummary) -> Result<(), AppError> {
        self.update(|state| state.credential = credential)
    }

    pub fn upsert_channel(&self, account: AccountSummary) -> Result<(), AppError> {
        self.update(|state| {
            state
                .channels
                .retain(|item| item.channel_id != account.channel_id);
            state.active_channel_id = Some(account.channel_id.clone());
            state.channels.push(account);
        })
    }

    pub fn set_active_channel(&self, channel_id: &str) -> Result<(), AppError> {
        self.update(|state| {
            if state
                .channels
                .iter()
                .any(|item| item.channel_id == channel_id)
            {
                state.active_channel_id = Some(channel_id.into());
            }
        })?;
        if self.snapshot().active_channel_id.as_deref() != Some(channel_id) {
            return Err(AppError::new(
                "YOUTUBE_CHANNEL_NOT_FOUND",
                "YouTube 频道不存在",
            ));
        }
        Ok(())
    }

    pub fn remove_channel(&self, channel_id: &str) -> Result<(), AppError> {
        self.update(|state| {
            state.channels.retain(|item| item.channel_id != channel_id);
            if state.active_channel_id.as_deref() == Some(channel_id) {
                state.active_channel_id =
                    state.channels.first().map(|item| item.channel_id.clone());
            }
        })
    }

    pub fn clear_credential(&self) -> Result<(), AppError> {
        self.update(|state| state.credential = CredentialSummary::default())
    }

    fn update(&self, apply: impl FnOnce(&mut PersistedState)) -> Result<(), AppError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::new("YOUTUBE_STATE_ERROR", "YouTube 状态不可用"))?;
        let mut next = guard.clone();
        apply(&mut next);
        persist(&self.path, &next)?;
        *guard = next;
        Ok(())
    }
}

fn persist(path: &Path, state: &PersistedState) -> Result<(), AppError> {
    let directory = path
        .parent()
        .ok_or_else(|| AppError::new("YOUTUBE_STATE_ERROR", "YouTube 状态路径无效"))?;
    fs::create_dir_all(directory).map_err(state_error)?;
    let temporary = path.with_extension("json.tmp");
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(state_error)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, state).map_err(state_error)?;
    writer
        .flush()
        .and_then(|_| writer.get_ref().sync_all())
        .map_err(state_error)?;
    drop(writer);
    fs::rename(&temporary, path).map_err(state_error)?;
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(state_error)
}

fn state_error(error: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "YOUTUBE_STATE_ERROR",
        "无法保存 YouTube 状态",
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_persists_only_safe_channel_summaries() {
        let root =
            std::env::temp_dir().join(format!("hongguo-youtube-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let store = YouTubeStateStore::load(&root).unwrap();
        store
            .set_credential(CredentialSummary {
                configured: true,
                client_id_suffix: "…123456".into(),
            })
            .unwrap();
        store
            .upsert_channel(AccountSummary {
                channel_id: "UC_SAFE".into(),
                title: "测试频道".into(),
                authorized_at: "2026-09-03T00:00:00Z".into(),
            })
            .unwrap();
        let bytes = fs::read(root.join("youtube/youtube-state.json")).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("token"));
        assert!(!text.contains("secret"));
        assert_eq!(
            YouTubeStateStore::load(&root)
                .unwrap()
                .snapshot()
                .active_channel_id
                .as_deref(),
            Some("UC_SAFE")
        );
        let _ = fs::remove_dir_all(root);
    }
}
