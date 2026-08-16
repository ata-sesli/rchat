use rchat_core::{
    chat::media::MediaKind,
    chat_identity::extract_peer_id_from_chat_id,
    chat_kind::{self, ChatKind},
    events::LocalPeerEvent,
    storage::config::{ConnectivityMode, ConnectivitySettings},
    storage::db::{ChatFileRow, Message},
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiChat {
    pub id: String,
    pub name: String,
    pub latest_timestamp: i64,
    pub unread_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiEnvelope {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiLocalPeer {
    pub peer_id: String,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiMessage {
    pub id: String,
    pub chat_id: String,
    pub sender: String,
    pub text: String,
    pub timestamp: i64,
    pub status: String,
    pub content_type: String,
    pub file_hash: Option<String>,
    pub content_metadata: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiChatFileSummary {
    pub message_id: String,
    pub content_type: String,
    pub display_name: String,
    pub size_bytes: Option<i64>,
    pub file_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiChatDetails {
    pub chat_id: String,
    pub peer_id: String,
    pub peer_name: String,
    pub peer_alias: Option<String>,
    pub avatar_url: Option<String>,
    pub connected: bool,
    pub remote_addr: Option<String>,
    pub reconnect_count: i64,
    pub sent_total: i64,
    pub received_total: i64,
    pub recent_files: Vec<TuiChatFileSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerDraft {
    pub chat_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncomingMessageEffect {
    AppendedToActive,
    StoredInactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    Chats,
    History,
    ComposerActions,
    Composer,
    CommandPalette,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerAction {
    Attach,
    Stickers,
    Voice,
    Video,
    Screen,
    Details,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuTarget {
    Chat(usize),
    Envelope(String),
    Message(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuAction {
    Open,
    Details,
    MoveToRoot,
    DeleteEnvelope,
    AttachmentActions,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuState {
    pub target: ContextMenuTarget,
    pub selected_index: usize,
}

impl ContextMenuState {
    pub fn new(target: ContextMenuTarget) -> Self {
        Self {
            target,
            selected_index: 0,
        }
    }

    pub fn move_selection(&mut self, delta: isize, len: usize) {
        self.selected_index = next_index(self.selected_index, delta, len);
    }
}

impl ComposerAction {
    pub const ALL: [Self; 6] = [
        Self::Attach,
        Self::Stickers,
        Self::Voice,
        Self::Video,
        Self::Screen,
        Self::Details,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Attach => "Attach",
            Self::Stickers => "Stickers",
            Self::Voice => "Voice",
            Self::Video => "Video",
            Self::Screen => "Screen",
            Self::Details => "Details",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewPersonStep {
    SelectNetwork,
    LocalScan,
    Online,
    TemporaryChat,
    CreateInviteUser,
    CreateInviteCode,
    AcceptInviteUser,
    AcceptInviteCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewPersonField {
    LocalNetwork,
    OnlineNetwork,
    LocalPeer(usize),
    CreateInvite,
    AcceptInvite,
    TemporaryChat,
    InviteeUsername,
    CreateInviteNext,
    CreateInviteConfirm,
    InviterUsername,
    AcceptInviteNext,
    InvitePassword,
    InviteQrPath,
    DecodeInviteQr,
    RedeemInvite,
    CreateTemporary,
    TemporaryLink,
    TemporaryQrPath,
    DecodeTemporaryQr,
    RedeemTemporary,
    CancelTemporary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPersonModalState {
    pub step: NewPersonStep,
    pub focus: NewPersonField,
    pub invitee_username: String,
    pub create_invite_password: String,
    pub inviter_username: String,
    pub invite_password: String,
    pub invite_qr_path: String,
    pub temporary_name: String,
    pub temporary_link: String,
    pub temporary_qr_path: String,
    pub active_temporary_link: Option<String>,
    pub active_temporary_remaining_seconds: Option<u64>,
    pub qr_payload: Option<String>,
    pub qr_error: Option<String>,
    pub error: Option<String>,
    pub waiting_peer_id: Option<String>,
}

impl Default for NewPersonModalState {
    fn default() -> Self {
        Self {
            step: NewPersonStep::SelectNetwork,
            focus: NewPersonField::LocalNetwork,
            invitee_username: String::new(),
            create_invite_password: String::new(),
            inviter_username: String::new(),
            invite_password: String::new(),
            invite_qr_path: String::new(),
            temporary_name: String::new(),
            temporary_link: String::new(),
            temporary_qr_path: String::new(),
            active_temporary_link: None,
            active_temporary_remaining_seconds: None,
            qr_payload: None,
            qr_error: None,
            error: None,
            waiting_peer_id: None,
        }
    }
}

impl NewPersonModalState {
    pub fn visible_fields(&self, local_peer_count: usize) -> Vec<NewPersonField> {
        match self.step {
            NewPersonStep::SelectNetwork => {
                vec![NewPersonField::LocalNetwork, NewPersonField::OnlineNetwork]
            }
            NewPersonStep::LocalScan => {
                let mut fields = (0..local_peer_count)
                    .map(NewPersonField::LocalPeer)
                    .collect::<Vec<_>>();
                if fields.is_empty() {
                    fields.push(NewPersonField::LocalNetwork);
                }
                fields
            }
            NewPersonStep::Online => vec![
                NewPersonField::CreateInvite,
                NewPersonField::AcceptInvite,
                NewPersonField::TemporaryChat,
            ],
            NewPersonStep::CreateInviteUser => {
                vec![
                    NewPersonField::InviteeUsername,
                    NewPersonField::CreateInviteNext,
                ]
            }
            NewPersonStep::CreateInviteCode => vec![NewPersonField::CreateInviteConfirm],
            NewPersonStep::AcceptInviteUser => {
                vec![
                    NewPersonField::InviterUsername,
                    NewPersonField::AcceptInviteNext,
                ]
            }
            NewPersonStep::AcceptInviteCode => vec![
                NewPersonField::InvitePassword,
                NewPersonField::InviteQrPath,
                NewPersonField::DecodeInviteQr,
                NewPersonField::RedeemInvite,
            ],
            NewPersonStep::TemporaryChat => vec![
                NewPersonField::CreateTemporary,
                NewPersonField::TemporaryLink,
                NewPersonField::TemporaryQrPath,
                NewPersonField::DecodeTemporaryQr,
                NewPersonField::RedeemTemporary,
                NewPersonField::CancelTemporary,
            ],
        }
    }

    pub fn set_step(&mut self, step: NewPersonStep) {
        self.step = step;
        self.error = None;
        self.qr_error = None;
        self.focus = match step {
            NewPersonStep::SelectNetwork => NewPersonField::LocalNetwork,
            NewPersonStep::LocalScan => NewPersonField::LocalPeer(0),
            NewPersonStep::Online => NewPersonField::CreateInvite,
            NewPersonStep::TemporaryChat => NewPersonField::CreateTemporary,
            NewPersonStep::CreateInviteUser => NewPersonField::InviteeUsername,
            NewPersonStep::CreateInviteCode => NewPersonField::CreateInviteConfirm,
            NewPersonStep::AcceptInviteUser => NewPersonField::InviterUsername,
            NewPersonStep::AcceptInviteCode => NewPersonField::InvitePassword,
        };
    }

    pub fn go_back(&mut self) -> bool {
        let previous = match self.step {
            NewPersonStep::SelectNetwork => return false,
            NewPersonStep::LocalScan | NewPersonStep::Online => NewPersonStep::SelectNetwork,
            NewPersonStep::TemporaryChat => NewPersonStep::Online,
            NewPersonStep::CreateInviteUser | NewPersonStep::AcceptInviteUser => {
                NewPersonStep::Online
            }
            NewPersonStep::CreateInviteCode => NewPersonStep::CreateInviteUser,
            NewPersonStep::AcceptInviteCode => NewPersonStep::AcceptInviteUser,
        };
        self.set_step(previous);
        true
    }

    pub fn cycle_focus(&mut self, local_peer_count: usize) {
        self.move_focus(1, local_peer_count);
    }

    pub fn move_focus(&mut self, delta: isize, local_peer_count: usize) {
        let fields = self.visible_fields(local_peer_count);
        if fields.is_empty() {
            return;
        }
        let current = fields
            .iter()
            .position(|field| field == &self.focus)
            .unwrap_or(0);
        self.focus = fields[next_index(current, delta, fields.len())].clone();
    }

    pub fn push_char(&mut self, ch: char) {
        match self.focus {
            NewPersonField::InviteeUsername => self.invitee_username.push(ch),
            NewPersonField::InviterUsername => self.inviter_username.push(ch),
            NewPersonField::InvitePassword => self.invite_password.push(ch),
            NewPersonField::InviteQrPath => self.invite_qr_path.push(ch),
            NewPersonField::TemporaryLink => self.temporary_link.push(ch),
            NewPersonField::TemporaryQrPath => self.temporary_qr_path.push(ch),
            _ => {}
        }
    }

    pub fn pop_char(&mut self) {
        match self.focus {
            NewPersonField::InviteeUsername => {
                self.invitee_username.pop();
            }
            NewPersonField::InviterUsername => {
                self.inviter_username.pop();
            }
            NewPersonField::InvitePassword => {
                self.invite_password.pop();
            }
            NewPersonField::InviteQrPath => {
                self.invite_qr_path.pop();
            }
            NewPersonField::TemporaryLink => {
                self.temporary_link.pop();
            }
            NewPersonField::TemporaryQrPath => {
                self.temporary_qr_path.pop();
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Profile,
    Peers,
    Connectivity,
    Theme,
    Stickers,
    Media,
    About,
}

impl SettingsSection {
    pub const ALL: [SettingsSection; 7] = [
        SettingsSection::Profile,
        SettingsSection::Peers,
        SettingsSection::Connectivity,
        SettingsSection::Theme,
        SettingsSection::Stickers,
        SettingsSection::Media,
        SettingsSection::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsSection::Profile => "Profile",
            SettingsSection::Peers => "Peers",
            SettingsSection::Connectivity => "Connectivity",
            SettingsSection::Theme => "Theme",
            SettingsSection::Stickers => "Stickers",
            SettingsSection::Media => "Media",
            SettingsSection::About => "About",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsField {
    Section(usize),
    ProfileAlias,
    ProfileAvatar,
    ProfileSave,
    Peer(usize),
    Friend(usize),
    ConnectivityMode(ConnectivityMode),
    ConnectivitySave,
    ThemePreset(usize),
    ThemeApply,
    ThemeName,
    ThemePrimary,
    ThemeSecondary,
    ThemeText,
    ThemeCreateCustom,
    Sticker(usize),
    StickerPath,
    StickerImport,
    StickerDelete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPane {
    Menu,
    Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiThemePreset {
    pub key: String,
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiSticker {
    pub file_hash: String,
    pub name: Option<String>,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsModalState {
    pub section: SettingsSection,
    pub pane: SettingsPane,
    pub focus: SettingsField,
    pub profile_alias: String,
    pub profile_avatar_path: String,
    pub trusted_peers: Vec<String>,
    pub friends: Vec<String>,
    pub pinned_peers: Vec<String>,
    pub connectivity: ConnectivitySettings,
    pub theme_presets: Vec<TuiThemePreset>,
    pub selected_preset: Option<String>,
    pub theme_custom_name: String,
    pub theme_primary: String,
    pub theme_secondary: String,
    pub theme_text: String,
    pub stickers: Vec<TuiSticker>,
    pub selected_sticker_hash: Option<String>,
    pub sticker_path: String,
    pub status: Option<String>,
    pub error: Option<String>,
}

impl Default for SettingsModalState {
    fn default() -> Self {
        Self {
            section: SettingsSection::Profile,
            pane: SettingsPane::Menu,
            focus: SettingsField::Section(0),
            profile_alias: String::new(),
            profile_avatar_path: String::new(),
            trusted_peers: Vec::new(),
            friends: Vec::new(),
            pinned_peers: Vec::new(),
            connectivity: ConnectivitySettings::default(),
            theme_presets: Vec::new(),
            selected_preset: None,
            theme_custom_name: String::new(),
            theme_primary: "#14b8a6".to_string(),
            theme_secondary: "#a855f7".to_string(),
            theme_text: "#e2e8f0".to_string(),
            stickers: Vec::new(),
            selected_sticker_hash: None,
            sticker_path: String::new(),
            status: None,
            error: None,
        }
    }
}

impl SettingsModalState {
    pub fn menu_fields(&self) -> Vec<SettingsField> {
        SettingsSection::ALL
            .iter()
            .enumerate()
            .map(|(index, _)| SettingsField::Section(index))
            .collect()
    }

    pub fn content_fields(&self) -> Vec<SettingsField> {
        let mut fields = Vec::new();
        match self.section {
            SettingsSection::Profile => fields.extend([
                SettingsField::ProfileAlias,
                SettingsField::ProfileAvatar,
                SettingsField::ProfileSave,
            ]),
            SettingsSection::Peers => {
                fields.extend(
                    (0..self.trusted_peers.len())
                        .take(6)
                        .map(SettingsField::Peer),
                );
                fields.extend((0..self.friends.len()).take(6).map(SettingsField::Friend));
            }
            SettingsSection::Connectivity => fields.extend([
                SettingsField::ConnectivityMode(ConnectivityMode::Invisible),
                SettingsField::ConnectivityMode(ConnectivityMode::Lan),
                SettingsField::ConnectivityMode(ConnectivityMode::Reachable),
                SettingsField::ConnectivitySave,
            ]),
            SettingsSection::Theme => {
                fields.extend(
                    (0..self.theme_presets.len())
                        .take(8)
                        .map(SettingsField::ThemePreset),
                );
                fields.extend([
                    SettingsField::ThemeApply,
                    SettingsField::ThemeName,
                    SettingsField::ThemePrimary,
                    SettingsField::ThemeSecondary,
                    SettingsField::ThemeText,
                    SettingsField::ThemeCreateCustom,
                ]);
            }
            SettingsSection::Stickers => {
                fields.extend((0..self.stickers.len()).take(8).map(SettingsField::Sticker));
                fields.extend([
                    SettingsField::StickerPath,
                    SettingsField::StickerImport,
                    SettingsField::StickerDelete,
                ]);
            }
            SettingsSection::Media | SettingsSection::About => {}
        }
        fields
    }

    pub fn visible_fields(&self) -> Vec<SettingsField> {
        match self.pane {
            SettingsPane::Menu => self.menu_fields(),
            SettingsPane::Content => self.content_fields(),
        }
    }

    pub fn cycle_focus(&mut self) {
        match self.pane {
            SettingsPane::Menu => self.focus_first_content_field(),
            SettingsPane::Content => {
                self.pane = SettingsPane::Menu;
                self.focus = self.section_field();
            }
        }
    }

    pub fn set_section(&mut self, section: SettingsSection) {
        self.section = section;
        self.pane = SettingsPane::Menu;
        self.status = None;
        self.error = None;
        self.focus = self.section_field();
    }

    pub fn activate_section(&mut self, section: SettingsSection) {
        self.section = section;
        self.status = None;
        self.error = None;
        self.focus_first_content_field();
    }

    pub fn move_section(&mut self, delta: isize) {
        let current = SettingsSection::ALL
            .iter()
            .position(|section| *section == self.section)
            .unwrap_or(0);
        let next = next_index(current, delta, SettingsSection::ALL.len());
        self.set_section(SettingsSection::ALL[next]);
    }

    pub fn move_content(&mut self, delta: isize) {
        let fields = self.content_fields();
        if fields.is_empty() {
            self.pane = SettingsPane::Menu;
            self.focus = self.section_field();
            return;
        }
        let current = fields
            .iter()
            .position(|field| field == &self.focus)
            .unwrap_or(0);
        self.pane = SettingsPane::Content;
        self.focus = fields[next_index(current, delta, fields.len())].clone();
    }

    fn focus_first_content_field(&mut self) {
        let fields = self.content_fields();
        if let Some(field) = fields.first() {
            self.pane = SettingsPane::Content;
            self.focus = field.clone();
        } else {
            self.pane = SettingsPane::Menu;
            self.focus = self.section_field();
        }
    }

    fn section_field(&self) -> SettingsField {
        SettingsField::Section(
            SettingsSection::ALL
                .iter()
                .position(|candidate| *candidate == self.section)
                .unwrap_or(0),
        )
    }

    pub fn focused_theme_preset_key(&self) -> Option<&str> {
        match self.focus {
            SettingsField::ThemePreset(index) => self
                .theme_presets
                .get(index)
                .map(|preset| preset.key.as_str()),
            _ => self
                .selected_preset
                .as_deref()
                .or_else(|| self.theme_presets.first().map(|preset| preset.key.as_str())),
        }
    }

    pub fn select_sticker(&mut self, index: usize) {
        self.selected_sticker_hash = self
            .stickers
            .get(index)
            .map(|sticker| sticker.file_hash.clone());
    }

    pub fn selected_sticker_hash(&self) -> Option<&str> {
        self.selected_sticker_hash.as_deref().filter(|hash| {
            self.stickers
                .iter()
                .any(|sticker| sticker.file_hash.as_str() == *hash)
        })
    }

    pub fn sticker_is_selected(&self, index: usize) -> bool {
        self.stickers
            .get(index)
            .zip(self.selected_sticker_hash())
            .is_some_and(|(sticker, hash)| sticker.file_hash == hash)
    }

    pub fn push_char(&mut self, ch: char) {
        match self.focus {
            SettingsField::ProfileAlias => self.profile_alias.push(ch),
            SettingsField::ProfileAvatar => self.profile_avatar_path.push(ch),
            SettingsField::ThemeName => self.theme_custom_name.push(ch),
            SettingsField::ThemePrimary => self.theme_primary.push(ch),
            SettingsField::ThemeSecondary => self.theme_secondary.push(ch),
            SettingsField::ThemeText => self.theme_text.push(ch),
            SettingsField::StickerPath => self.sticker_path.push(ch),
            _ => {}
        }
    }

    pub fn pop_char(&mut self) {
        match self.focus {
            SettingsField::ProfileAlias => {
                self.profile_alias.pop();
            }
            SettingsField::ProfileAvatar => {
                self.profile_avatar_path.pop();
            }
            SettingsField::ThemeName => {
                self.theme_custom_name.pop();
            }
            SettingsField::ThemePrimary => {
                self.theme_primary.pop();
            }
            SettingsField::ThemeSecondary => {
                self.theme_secondary.pop();
            }
            SettingsField::ThemeText => {
                self.theme_text.pop();
            }
            SettingsField::StickerPath => {
                self.sticker_path.pop();
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentModalField {
    Kind,
    Picker,
    Send,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentFileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentModalState {
    pub kind: MediaKind,
    pub path: String,
    pub picker_root: PathBuf,
    pub picker_query: String,
    pub picker_entries: Vec<AttachmentFileEntry>,
    pub selected_entry_index: usize,
    pub focus: AttachmentModalField,
    pub status: Option<String>,
    pub error: Option<String>,
}

impl Default for AttachmentModalState {
    fn default() -> Self {
        Self {
            kind: MediaKind::Image,
            path: String::new(),
            picker_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            picker_query: String::new(),
            picker_entries: Vec::new(),
            selected_entry_index: 0,
            focus: AttachmentModalField::Picker,
            status: None,
            error: None,
        }
    }
}

impl AttachmentModalState {
    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            AttachmentModalField::Kind => AttachmentModalField::Picker,
            AttachmentModalField::Picker => AttachmentModalField::Send,
            AttachmentModalField::Send => AttachmentModalField::Kind,
        };
    }

    pub fn cycle_kind(&mut self, delta: isize) {
        let kinds = [
            MediaKind::Image,
            MediaKind::Document,
            MediaKind::Video,
            MediaKind::Audio,
        ];
        let current = kinds
            .iter()
            .position(|kind| *kind == self.kind)
            .unwrap_or(0);
        self.kind = kinds[next_index(current, delta, kinds.len())];
    }

    pub fn push_char(&mut self, ch: char) {
        if self.focus == AttachmentModalField::Picker {
            self.picker_query.push(ch);
            self.selected_entry_index = 0;
        }
    }

    pub fn pop_char(&mut self) {
        if self.focus == AttachmentModalField::Picker {
            self.picker_query.pop();
            self.selected_entry_index = 0;
        }
    }

    pub fn set_path(&mut self, path: PathBuf) {
        self.path = path.display().to_string();
        self.status = Some("file selected".to_string());
        self.error = None;
    }

    pub fn set_picker_entries(&mut self, entries: Vec<AttachmentFileEntry>) {
        self.picker_entries = entries;
        if self.selected_entry_index >= self.visible_entries().len() {
            self.selected_entry_index = 0;
        }
    }

    pub fn move_entry_selection(&mut self, delta: isize) {
        let len = self.visible_entries().len();
        self.selected_entry_index = next_index(self.selected_entry_index, delta, len);
    }

    pub fn selected_entry(&self) -> Option<&AttachmentFileEntry> {
        self.visible_entries()
            .get(self.selected_entry_index)
            .copied()
    }

    pub fn visible_entries(&self) -> Vec<&AttachmentFileEntry> {
        let query = self.picker_query.trim().to_lowercase();
        let mut entries = self
            .picker_entries
            .iter()
            .filter(|entry| query.is_empty() || fuzzy_match(&entry.name, &query))
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| (!entry.is_dir, entry.name.to_lowercase()));
        entries
    }

    pub fn selected_preview_path(&self) -> Option<&str> {
        if !self.path.trim().is_empty() {
            return Some(self.path.trim());
        }
        self.selected_entry()
            .filter(|entry| !entry.is_dir)
            .map(|entry| entry.path.to_str())
            .flatten()
    }
}

fn fuzzy_match(candidate: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let candidate = candidate.to_lowercase();
    if candidate.contains(query) {
        return true;
    }
    let mut chars = candidate.chars();
    query
        .chars()
        .all(|needle| chars.by_ref().any(|candidate| candidate == needle))
}

#[cfg(test)]
pub fn attachment_file_entry(path: impl Into<PathBuf>, is_dir: bool) -> AttachmentFileEntry {
    let path = path.into();
    AttachmentFileEntry {
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string(),
        path,
        is_dir,
        size_bytes: None,
    }
}

#[cfg(test)]
fn attachment_file_entry_named(
    name: impl Into<String>,
    path: impl Into<PathBuf>,
    is_dir: bool,
) -> AttachmentFileEntry {
    AttachmentFileEntry {
        path: path.into(),
        name: name.into(),
        is_dir,
        size_bytes: None,
    }
}

#[cfg(test)]
pub fn attachment_entry_matches_query(name: &str, query: &str) -> bool {
    fuzzy_match(name, query)
}

impl StickerPickerState {
    pub fn selected_sticker(&self) -> Option<&TuiSticker> {
        self.stickers.get(self.selected_index)
    }

    pub fn selected_preview_message(&self) -> Option<TuiMessage> {
        let sticker = self.selected_sticker()?;
        Some(TuiMessage {
            id: format!("sticker-preview-{}", sticker.file_hash),
            chat_id: "sticker-picker".to_string(),
            sender: "Me".to_string(),
            text: sticker.name.clone().unwrap_or_else(|| "sticker".to_string()),
            timestamp: 0,
            status: String::new(),
            content_type: "sticker".to_string(),
            file_hash: Some(sticker.file_hash.clone()),
            content_metadata: None,
        })
    }
}

impl AttachmentModalState {
    pub fn selected_path_or_entry(&self) -> Option<PathBuf> {
        if !self.path.trim().is_empty() {
            return Some(PathBuf::from(self.path.trim()));
        }
        self.selected_entry()
            .filter(|entry| !entry.is_dir)
            .map(|entry| entry.path.clone())
    }
}

impl AttachmentModalState {
    pub fn clear_selection(&mut self) {
        self.path.clear();
        self.status = None;
        self.error = None;
    }

    pub fn enter_directory(&mut self, path: PathBuf) {
        self.picker_root = path;
        self.picker_query.clear();
        self.selected_entry_index = 0;
        self.clear_selection();
    }

    pub fn go_parent(&mut self) {
        if let Some(parent) = self.picker_root.parent() {
            self.enter_directory(parent.to_path_buf());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StickerPickerMode {
    Browse,
    AddPath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickerPickerState {
    pub stickers: Vec<TuiSticker>,
    pub selected_index: usize,
    pub mode: StickerPickerMode,
    pub add_path: String,
    pub status: Option<String>,
    pub error: Option<String>,
}

impl StickerPickerState {
    pub fn new(stickers: Vec<TuiSticker>) -> Self {
        Self {
            stickers,
            selected_index: 0,
            mode: StickerPickerMode::Browse,
            add_path: String::new(),
            status: None,
            error: None,
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        self.selected_index = next_index(self.selected_index, delta, self.stickers.len());
    }

    pub fn selected_hash(&self) -> Option<&str> {
        self.stickers
            .get(self.selected_index)
            .map(|sticker| sticker.file_hash.as_str())
    }

    pub fn select_hash(&mut self, file_hash: &str) {
        if let Some(index) = self
            .stickers
            .iter()
            .position(|sticker| sticker.file_hash == file_hash)
        {
            self.selected_index = index;
        }
    }

    pub fn enter_add_path_mode(&mut self) {
        self.mode = StickerPickerMode::AddPath;
        self.add_path.clear();
        self.status = Some("enter sticker image path".to_string());
        self.error = None;
    }

    pub fn exit_add_path_mode(&mut self) {
        self.mode = StickerPickerMode::Browse;
        self.add_path.clear();
        self.status = None;
        self.error = None;
    }

    pub fn push_char(&mut self, ch: char) {
        if self.mode == StickerPickerMode::AddPath {
            self.add_path.push(ch);
        }
    }

    pub fn pop_char(&mut self) {
        if self.mode == StickerPickerMode::AddPath {
            self.add_path.pop();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaViewerKind {
    Image,
    Video,
    Audio,
    Document,
}

impl MediaViewerKind {
    pub fn from_content_type(content_type: &str) -> Self {
        match content_type {
            "image" | "photo" | "sticker" => Self::Image,
            "video" => Self::Video,
            "audio" => Self::Audio,
            "document" => Self::Document,
            _ => Self::Document,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Document => "document",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaViewerAction {
    SavePath,
    Save,
    Open,
    CopyHash,
    Retry,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaViewerState {
    pub message_id: String,
    pub file_hash: String,
    pub file_name: String,
    pub content_type: String,
    pub metadata: Option<String>,
    pub kind: MediaViewerKind,
    pub zoom_percent: u16,
    pub pan_x: i32,
    pub pan_y: i32,
    pub target_path: String,
    pub focus: MediaViewerAction,
    pub status: Option<String>,
    pub error: Option<String>,
}

impl MediaViewerState {
    pub fn new(
        message_id: String,
        file_hash: String,
        file_name: String,
        content_type: String,
        metadata: Option<String>,
    ) -> Self {
        let kind = MediaViewerKind::from_content_type(&content_type);
        Self {
            message_id,
            file_hash,
            file_name,
            content_type,
            metadata,
            kind,
            zoom_percent: 100,
            pan_x: 0,
            pan_y: 0,
            target_path: String::new(),
            focus: MediaViewerAction::Open,
            status: None,
            error: None,
        }
    }

    pub fn from_message(message: &TuiMessage) -> Option<Self> {
        Some(Self::new(
            message.id.clone(),
            message.file_hash.clone()?,
            attachment_display_name(message),
            message.content_type.clone(),
            message.content_metadata.clone(),
        ))
        .filter(|_| message_is_attachment(message))
    }

    pub fn zoom_in(&mut self) {
        self.zoom_percent = self.zoom_percent.saturating_add(25).min(400);
    }

    pub fn zoom_out(&mut self) {
        self.zoom_percent = self.zoom_percent.saturating_sub(25).max(25);
        if self.zoom_percent <= 100 {
            self.pan_x = 0;
            self.pan_y = 0;
        }
    }

    pub fn pan_by(&mut self, x: i32, y: i32) -> bool {
        let previous = (self.pan_x, self.pan_y);
        if self.zoom_percent <= 100 {
            self.pan_x = 0;
            self.pan_y = 0;
            return previous != (self.pan_x, self.pan_y);
        }

        self.pan_x = self.pan_x.saturating_add(x);
        self.pan_y = self.pan_y.saturating_add(y);
        previous != (self.pan_x, self.pan_y)
    }

    pub fn reset_view(&mut self) {
        self.zoom_percent = 100;
        self.pan_x = 0;
        self.pan_y = 0;
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            MediaViewerAction::SavePath => MediaViewerAction::Save,
            MediaViewerAction::Save => MediaViewerAction::Open,
            MediaViewerAction::Open => MediaViewerAction::CopyHash,
            MediaViewerAction::CopyHash => MediaViewerAction::Retry,
            MediaViewerAction::Retry => MediaViewerAction::Close,
            MediaViewerAction::Close => MediaViewerAction::SavePath,
        };
    }

    pub fn push_char(&mut self, ch: char) {
        if self.focus == MediaViewerAction::SavePath {
            self.target_path.push(ch);
        }
    }

    pub fn pop_char(&mut self) {
        if self.focus == MediaViewerAction::SavePath {
            self.target_path.pop();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentActionField {
    View,
    SavePath,
    Save,
    Open,
    CopyHash,
    Retry,
    SaveSticker,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentActionModalState {
    pub message_id: String,
    pub file_hash: String,
    pub file_name: String,
    pub content_type: String,
    pub target_path: String,
    pub sticker_saved: bool,
    pub focus: AttachmentActionField,
    pub status: Option<String>,
    pub error: Option<String>,
}

impl AttachmentActionModalState {
    pub fn from_message(message: &TuiMessage) -> Option<Self> {
        Self::from_message_with_sticker_saved(message, false)
    }

    pub fn from_message_with_sticker_saved(
        message: &TuiMessage,
        sticker_saved: bool,
    ) -> Option<Self> {
        let file_hash = message.file_hash.clone()?;
        if file_hash.trim().is_empty() || !message_is_attachment(message) {
            return None;
        }
        Some(Self {
            message_id: message.id.clone(),
            file_hash,
            file_name: attachment_display_name(message),
            content_type: message.content_type.clone(),
            target_path: String::new(),
            sticker_saved,
            focus: AttachmentActionField::View,
            status: None,
            error: None,
        })
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            AttachmentActionField::View => AttachmentActionField::SavePath,
            AttachmentActionField::SavePath => AttachmentActionField::Save,
            AttachmentActionField::Save => AttachmentActionField::Open,
            AttachmentActionField::Open => AttachmentActionField::CopyHash,
            AttachmentActionField::CopyHash => AttachmentActionField::Retry,
            AttachmentActionField::Retry
                if self.content_type == "sticker" && !self.sticker_saved =>
            {
                AttachmentActionField::SaveSticker
            }
            AttachmentActionField::Retry => AttachmentActionField::Close,
            AttachmentActionField::SaveSticker => AttachmentActionField::Close,
            AttachmentActionField::Close => AttachmentActionField::View,
        };
    }

    pub fn push_char(&mut self, ch: char) {
        if self.focus == AttachmentActionField::SavePath {
            self.target_path.push(ch);
        }
    }

    pub fn pop_char(&mut self) {
        if self.focus == AttachmentActionField::SavePath {
            self.target_path.pop();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppSessionPhase {
    Checking,
    Locked,
    Unlocked,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiAppState {
    pub session_phase: AppSessionPhase,
    pub app_ready: bool,
    pub status: String,
    pub chats: Vec<TuiChat>,
    pub envelopes: Vec<TuiEnvelope>,
    pub envelope_assignments: HashMap<String, String>,
    pub selected_chat_index: usize,
    pub sidebar_scroll_offset: usize,
    pub active_chat_id: Option<String>,
    pub active_conversation_ids: HashSet<String>,
    pub connected_chat_ids: HashSet<String>,
    pub pinned_chat_keys: HashSet<String>,
    pub local_peers: Vec<TuiLocalPeer>,
    pub messages: Vec<TuiMessage>,
    pub history_scroll_offset: usize,
    pub selected_message_id: Option<String>,
    pub composer: String,
    pub selected_composer_action_index: usize,
    pub sidebar_search: String,
    pub sidebar_search_active: bool,
    pub command_input: String,
    pub focus: FocusPane,
    pub show_help: bool,
    pub show_command_palette: bool,
    pub new_person: Option<NewPersonModalState>,
    pub settings: Option<SettingsModalState>,
    pub attachment_modal: Option<AttachmentModalState>,
    pub sticker_picker: Option<StickerPickerState>,
    pub attachment_actions: Option<AttachmentActionModalState>,
    pub context_menu: Option<ContextMenuState>,
    pub media_viewer: Option<MediaViewerState>,
    pub selected_attachment_message_id: Option<String>,
    pub chat_details: Option<TuiChatDetails>,
    pub last_error: Option<String>,
}

impl Default for TuiAppState {
    fn default() -> Self {
        Self {
            session_phase: AppSessionPhase::Checking,
            app_ready: false,
            status: "starting".to_string(),
            chats: Vec::new(),
            envelopes: Vec::new(),
            envelope_assignments: HashMap::new(),
            selected_chat_index: 0,
            sidebar_scroll_offset: 0,
            active_chat_id: None,
            active_conversation_ids: HashSet::new(),
            connected_chat_ids: HashSet::new(),
            pinned_chat_keys: HashSet::new(),
            local_peers: Vec::new(),
            messages: Vec::new(),
            history_scroll_offset: 0,
            selected_message_id: None,
            composer: String::new(),
            selected_composer_action_index: 0,
            sidebar_search: String::new(),
            sidebar_search_active: false,
            command_input: String::new(),
            focus: FocusPane::Chats,
            show_help: false,
            show_command_palette: false,
            new_person: None,
            settings: None,
            attachment_modal: None,
            sticker_picker: None,
            attachment_actions: None,
            context_menu: None,
            media_viewer: None,
            selected_attachment_message_id: None,
            chat_details: None,
            last_error: None,
        }
    }
}

impl TuiAppState {
    pub fn replace_chats(&mut self, mut chats: Vec<TuiChat>) {
        chats.sort_by(|a, b| {
            b.latest_timestamp
                .cmp(&a.latest_timestamp)
                .then_with(|| a.name.cmp(&b.name))
        });
        self.chats = chats;
        if self.selected_chat_index >= self.chats.len() {
            self.selected_chat_index = self.chats.len().saturating_sub(1);
        }
    }

    pub fn replace_envelopes(&mut self, mut envelopes: Vec<TuiEnvelope>) {
        envelopes.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        self.envelopes = envelopes;
    }

    pub fn replace_envelope_assignments(&mut self, assignments: HashMap<String, String>) {
        self.envelope_assignments = assignments;
    }

    pub fn chat_envelope_id(&self, chat_id: &str) -> Option<&str> {
        self.envelope_assignments
            .get(&normalize_chat_id(chat_id))
            .map(String::as_str)
    }

    pub fn selected_chat_id(&self) -> Option<&str> {
        self.chats
            .get(self.selected_chat_index)
            .map(|chat| chat.id.as_str())
            .or(self.active_chat_id.as_deref())
    }

    pub fn move_selection(&mut self, delta: isize) {
        match self.focus {
            FocusPane::History => self.move_message_selection(delta),
            FocusPane::ComposerActions => self.move_composer_action(delta),
            _ => {
                self.selected_chat_index =
                    next_index(self.selected_chat_index, delta, self.chats.len());
            }
        }
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            FocusPane::Chats => FocusPane::History,
            FocusPane::History => FocusPane::ComposerActions,
            FocusPane::ComposerActions => FocusPane::Composer,
            FocusPane::Composer | FocusPane::CommandPalette => FocusPane::Chats,
        };
    }

    pub fn selected_composer_action(&self) -> ComposerAction {
        ComposerAction::ALL
            .get(self.selected_composer_action_index)
            .copied()
            .unwrap_or(ComposerAction::Attach)
    }

    pub fn move_composer_action(&mut self, delta: isize) {
        let len = ComposerAction::ALL.len();
        if len == 0 {
            self.selected_composer_action_index = 0;
            return;
        }
        let current = self.selected_composer_action_index as isize;
        self.selected_composer_action_index =
            (current + delta).rem_euclid(len as isize) as usize;
    }

    pub fn open_sidebar_search(&mut self) {
        self.sidebar_search_active = true;
        self.focus = FocusPane::Chats;
    }

    pub fn close_sidebar_search(&mut self) {
        self.sidebar_search_active = false;
        self.sidebar_search.clear();
        self.sidebar_scroll_offset = 0;
    }

    pub fn push_sidebar_search_char(&mut self, ch: char) {
        self.sidebar_search_active = true;
        self.sidebar_search.push(ch);
        self.sidebar_scroll_offset = 0;
    }

    pub fn pop_sidebar_search_char(&mut self) {
        self.sidebar_search.pop();
        self.sidebar_scroll_offset = 0;
    }

    pub fn scroll_history(&mut self, delta: isize) {
        if self.messages.is_empty() {
            self.history_scroll_offset = 0;
        } else if delta >= 0 {
            self.history_scroll_offset = self.history_scroll_offset.saturating_add(delta as usize);
        } else {
            self.history_scroll_offset = self
                .history_scroll_offset
                .saturating_sub(delta.unsigned_abs());
        }
        self.clamp_history_scroll();
        self.sync_selected_message_to_history_scroll();
    }

    pub fn open_command_palette(&mut self) {
        self.show_command_palette = true;
        self.command_input.clear();
        self.focus = FocusPane::CommandPalette;
    }

    pub fn close_command_palette(&mut self) {
        self.show_command_palette = false;
        self.command_input.clear();
        self.focus = FocusPane::Chats;
    }

    pub fn close_modal(&mut self) {
        self.chat_details = None;
        self.show_help = false;
        self.new_person = None;
        self.settings = None;
        self.attachment_modal = None;
        self.sticker_picker = None;
        self.attachment_actions = None;
        self.context_menu = None;
        self.media_viewer = None;
    }

    pub fn open_new_person(&mut self) {
        self.show_command_palette = false;
        self.sidebar_search_active = false;
        self.chat_details = None;
        self.show_help = false;
        self.settings = None;
        self.attachment_modal = None;
        self.sticker_picker = None;
        self.attachment_actions = None;
        self.context_menu = None;
        self.media_viewer = None;
        self.new_person = Some(NewPersonModalState::default());
    }

    pub fn close_new_person(&mut self) {
        self.new_person = None;
    }

    pub fn open_settings(&mut self) {
        self.show_command_palette = false;
        self.sidebar_search_active = false;
        self.chat_details = None;
        self.show_help = false;
        self.new_person = None;
        self.attachment_modal = None;
        self.sticker_picker = None;
        self.attachment_actions = None;
        self.context_menu = None;
        self.media_viewer = None;
        self.settings = Some(SettingsModalState::default());
    }

    pub fn close_settings(&mut self) {
        self.settings = None;
    }

    pub fn open_attachment_modal(&mut self) {
        self.show_command_palette = false;
        self.sidebar_search_active = false;
        self.chat_details = None;
        self.show_help = false;
        self.new_person = None;
        self.settings = None;
        self.sticker_picker = None;
        self.attachment_actions = None;
        self.context_menu = None;
        self.media_viewer = None;
        self.attachment_modal = Some(AttachmentModalState::default());
    }

    pub fn open_sticker_picker(&mut self, stickers: Vec<TuiSticker>) {
        self.show_command_palette = false;
        self.sidebar_search_active = false;
        self.chat_details = None;
        self.show_help = false;
        self.new_person = None;
        self.settings = None;
        self.attachment_modal = None;
        self.attachment_actions = None;
        self.media_viewer = None;
        self.sticker_picker = Some(StickerPickerState::new(stickers));
    }

    pub fn open_attachment_actions_for_selected(&mut self) -> bool {
        let Some(message) = self.selected_attachment_message().cloned() else {
            self.last_error = Some("no attachment selected".to_string());
            return false;
        };
        let Some(modal) = AttachmentActionModalState::from_message(&message) else {
            self.last_error = Some("selected message has no attachment".to_string());
            return false;
        };
        self.show_command_palette = false;
        self.sidebar_search_active = false;
        self.chat_details = None;
        self.show_help = false;
        self.new_person = None;
        self.settings = None;
        self.attachment_modal = None;
        self.sticker_picker = None;
        self.context_menu = None;
        self.media_viewer = None;
        self.attachment_actions = Some(modal);
        true
    }

    pub fn open_media_viewer_for_selected(&mut self) -> bool {
        let Some(message) = self.selected_attachment_message().cloned() else {
            self.last_error = Some("no attachment selected".to_string());
            return false;
        };
        self.open_media_viewer_for_message(&message)
    }

    pub fn open_media_viewer_for_hash(&mut self, file_hash: &str) -> bool {
        let Some(message) = self
            .messages
            .iter()
            .find(|message| message.file_hash.as_deref() == Some(file_hash))
            .cloned()
        else {
            self.last_error = Some("attachment is not in the active chat".to_string());
            return false;
        };
        self.open_media_viewer_for_message(&message)
    }

    fn open_media_viewer_for_message(&mut self, message: &TuiMessage) -> bool {
        let Some(viewer) = MediaViewerState::from_message(message) else {
            self.last_error = Some("selected message has no attachment".to_string());
            return false;
        };
        self.show_command_palette = false;
        self.chat_details = None;
        self.show_help = false;
        self.new_person = None;
        self.settings = None;
        self.attachment_modal = None;
        self.sticker_picker = None;
        self.attachment_actions = None;
        self.selected_attachment_message_id = Some(message.id.clone());
        self.media_viewer = Some(viewer);
        true
    }

    pub fn selected_attachment_message(&self) -> Option<&TuiMessage> {
        if let Some(message) = self.selected_message() {
            return message_is_attachment(message).then_some(message);
        }

        self.selected_attachment_message_id
            .as_ref()
            .and_then(|id| self.messages.iter().find(|message| message.id == *id))
            .filter(|message| message_is_attachment(message))
    }

    pub fn move_attachment_selection(&mut self, delta: isize) {
        let attachments = self
            .messages
            .iter()
            .filter(|message| message_is_attachment(message))
            .map(|message| message.id.clone())
            .collect::<Vec<_>>();
        if attachments.is_empty() {
            self.selected_attachment_message_id = None;
            return;
        }
        let current = self
            .selected_attachment_message_id
            .as_ref()
            .and_then(|id| attachments.iter().position(|candidate| candidate == id))
            .unwrap_or_else(|| attachments.len().saturating_sub(1));
        let selected_id = attachments[next_index(current, delta, attachments.len())].clone();
        self.selected_attachment_message_id = Some(selected_id.clone());
        self.selected_message_id = Some(selected_id);
        self.sync_history_scroll_to_selected_message();
    }

    pub fn selected_message(&self) -> Option<&TuiMessage> {
        self.selected_message_id
            .as_ref()
            .and_then(|id| self.messages.iter().find(|message| message.id == *id))
            .or_else(|| self.messages.last())
    }

    pub fn move_message_selection(&mut self, delta: isize) {
        if self.messages.is_empty() {
            self.selected_message_id = None;
            self.selected_attachment_message_id = None;
            self.history_scroll_offset = 0;
            return;
        }

        let current = self
            .selected_message_id
            .as_ref()
            .and_then(|id| self.messages.iter().position(|message| message.id == *id))
            .unwrap_or_else(|| self.messages.len().saturating_sub(1));
        let next = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current
                .saturating_add(delta as usize)
                .min(self.messages.len().saturating_sub(1))
        };
        self.select_message_index(next);
        self.sync_history_scroll_to_selected_message();
    }

    pub fn apply_local_peer_discovered(&mut self, peer: LocalPeerEvent) {
        match self
            .local_peers
            .iter_mut()
            .find(|item| item.peer_id == peer.peer_id)
        {
            Some(existing) => existing.addresses = peer.addresses,
            None => self.local_peers.push(TuiLocalPeer {
                peer_id: peer.peer_id,
                addresses: peer.addresses,
            }),
        }
    }

    pub fn apply_local_peer_expired(&mut self, peer_id: &str) {
        self.local_peers.retain(|peer| peer.peer_id != peer_id);
    }

    pub fn apply_connected_chat_ids(&mut self, ids: Vec<String>) {
        self.connected_chat_ids = ids.into_iter().map(normalize_chat_id).collect();
    }

    pub fn is_chat_connected(&self, chat_id: &str) -> bool {
        let target = presence_key(chat_id);
        self.connected_chat_ids
            .iter()
            .any(|connected| presence_key(connected) == target)
    }

    pub fn is_chat_pinned(&self, chat: &TuiChat) -> bool {
        self.pinned_chat_keys.contains(&chat.id) || self.pinned_chat_keys.contains(&chat.name)
    }

    pub fn select_chat_with_history(&mut self, chat_id: &str, history: Vec<Message>) {
        let normalized = normalize_chat_id(chat_id);
        self.active_chat_id = Some(normalized.clone());
        self.active_conversation_ids.clear();
        self.active_conversation_ids.insert(normalized.clone());
        self.messages = history
            .into_iter()
            .map(|message| {
                let canonical = normalize_chat_id(&message.chat_id);
                self.active_conversation_ids.insert(canonical);
                TuiMessage::from(message)
            })
            .collect();
        self.history_scroll_offset = 0;
        self.selected_message_id = self.newest_message_id();
        self.selected_attachment_message_id = self.newest_attachment_message_id();
        self.media_viewer = None;

        if let Some(chat) = self.chats.iter_mut().find(|chat| chat.id == normalized) {
            chat.unread_count = 0;
        }
        if let Some(index) = self.chats.iter().position(|chat| chat.id == normalized) {
            self.selected_chat_index = index;
        }
    }

    pub fn unread_count(&self, chat_id: &str) -> i64 {
        self.chats
            .iter()
            .find(|chat| chat.id == normalize_chat_id(chat_id))
            .map(|chat| chat.unread_count)
            .unwrap_or(0)
    }

    pub fn apply_incoming_message(&mut self, message: Message) -> IncomingMessageEffect {
        let related_chat_id = normalize_chat_id(&message.chat_id);
        let related_key = presence_key(&related_chat_id);
        let is_active = self
            .active_chat_id
            .as_deref()
            .map(|active| presence_key(active) == related_key)
            .unwrap_or(false)
            || self.active_conversation_ids.contains(&related_chat_id);

        self.upsert_chat_activity(&related_chat_id, message.timestamp, !is_active);
        if is_active {
            let tui_message = TuiMessage::from(message);
            let appended_message_id = tui_message.id.clone();
            let appended_attachment_id =
                message_is_attachment(&tui_message).then(|| tui_message.id.clone());
            self.messages.push(tui_message);
            if self.history_scroll_offset > 0 {
                self.history_scroll_offset += 1;
                self.clamp_history_scroll();
            } else {
                self.selected_message_id = Some(appended_message_id);
                self.selected_attachment_message_id = appended_attachment_id;
            }
            IncomingMessageEffect::AppendedToActive
        } else {
            IncomingMessageEffect::StoredInactive
        }
    }

    pub fn apply_message_status(&mut self, msg_id: &str, status: &str) {
        for message in &mut self.messages {
            if message.id == msg_id {
                message.status = status.to_string();
            }
        }
    }

    pub fn prepare_composer_send(&self) -> Option<ComposerDraft> {
        let text = self.composer.trim();
        if text.is_empty() {
            return None;
        }
        let chat_id = self.active_chat_id.clone()?;
        Some(ComposerDraft {
            chat_id,
            text: text.to_string(),
        })
    }

    pub fn mark_composer_send_succeeded(
        &mut self,
        draft: ComposerDraft,
        msg_id: String,
        timestamp: i64,
    ) {
        self.composer.clear();
        self.selected_message_id = Some(msg_id.clone());
        self.messages.push(TuiMessage {
            id: msg_id,
            chat_id: draft.chat_id.clone(),
            sender: "Me".to_string(),
            text: draft.text,
            timestamp,
            status: outgoing_status(&draft.chat_id).to_string(),
            content_type: "text".to_string(),
            file_hash: None,
            content_metadata: None,
        });
        self.history_scroll_offset = 0;
        self.selected_attachment_message_id = None;
        self.upsert_chat_activity(&draft.chat_id, timestamp, false);
    }

    pub fn mark_composer_send_failed(&mut self, message: impl Into<String>) {
        self.last_error = Some(message.into());
    }

    fn upsert_chat_activity(&mut self, chat_id: &str, timestamp: i64, unread: bool) {
        let chat_id = normalize_chat_id(chat_id);
        if let Some(chat) = self.chats.iter_mut().find(|chat| chat.id == chat_id) {
            chat.latest_timestamp = chat.latest_timestamp.max(timestamp);
            if unread {
                chat.unread_count += 1;
            }
            return;
        }

        self.chats.push(TuiChat {
            id: chat_id.clone(),
            name: chat_id,
            latest_timestamp: timestamp,
            unread_count: i64::from(unread),
        });
    }

    fn clamp_history_scroll(&mut self) {
        self.history_scroll_offset = self
            .history_scroll_offset
            .min(self.messages.len().saturating_sub(1));
    }

    fn sync_selected_message_to_history_scroll(&mut self) {
        if self.messages.is_empty() {
            self.selected_message_id = None;
            self.selected_attachment_message_id = None;
            return;
        }
        let index = self
            .messages
            .len()
            .saturating_sub(self.history_scroll_offset.saturating_add(1));
        self.select_message_index(index);
    }

    fn sync_history_scroll_to_selected_message(&mut self) {
        let Some(index) = self.selected_message_index() else {
            self.history_scroll_offset = 0;
            return;
        };
        self.history_scroll_offset = self.messages.len().saturating_sub(index.saturating_add(1));
        self.clamp_history_scroll();
    }

    fn select_message_index(&mut self, index: usize) {
        let Some(message) = self.messages.get(index) else {
            self.selected_message_id = None;
            self.selected_attachment_message_id = None;
            return;
        };
        self.selected_message_id = Some(message.id.clone());
        self.selected_attachment_message_id = message_is_attachment(message).then(|| message.id.clone());
    }

    fn selected_message_index(&self) -> Option<usize> {
        self.selected_message_id
            .as_ref()
            .and_then(|id| self.messages.iter().position(|message| message.id == *id))
    }

    fn newest_attachment_message_id(&self) -> Option<String> {
        self.messages
            .iter()
            .rev()
            .find(|message| message_is_attachment(message))
            .map(|message| message.id.clone())
    }

    fn newest_message_id(&self) -> Option<String> {
        self.messages.last().map(|message| message.id.clone())
    }
}

impl From<Message> for TuiMessage {
    fn from(message: Message) -> Self {
        Self {
            id: message.id,
            chat_id: normalize_chat_id(&message.chat_id),
            sender: if message.peer_id == "Me" {
                "Me".to_string()
            } else {
                message.sender_alias.unwrap_or(message.peer_id)
            },
            text: message.text_content.unwrap_or_default(),
            timestamp: message.timestamp,
            status: message.status,
            content_type: message.content_type,
            file_hash: message.file_hash,
            content_metadata: message.content_metadata,
        }
    }
}

impl From<ChatFileRow> for TuiChatFileSummary {
    fn from(row: ChatFileRow) -> Self {
        Self {
            message_id: row.message_id,
            content_type: row.content_type,
            display_name: row
                .file_name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "attachment".to_string()),
            size_bytes: row.size_bytes,
            file_hash: row.file_hash,
        }
    }
}

pub fn normalize_chat_id(chat_id: impl AsRef<str>) -> String {
    match chat_id.as_ref() {
        "self" => "Me".to_string(),
        other => other.to_string(),
    }
}

pub fn db_chat_id(chat_id: &str) -> String {
    if chat_id == "Me" {
        "self".to_string()
    } else {
        chat_id.to_string()
    }
}

pub fn message_is_attachment(message: &TuiMessage) -> bool {
    matches!(
        message.content_type.as_str(),
        "image" | "photo" | "sticker" | "video" | "audio" | "document"
    ) && message
        .file_hash
        .as_deref()
        .is_some_and(|hash| !hash.is_empty())
}

pub fn attachment_display_name(message: &TuiMessage) -> String {
    let trimmed = message.text.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    match message.content_type.as_str() {
        "image" | "photo" => "image attachment".to_string(),
        "sticker" => "sticker".to_string(),
        "video" => "video attachment".to_string(),
        "audio" => "audio attachment".to_string(),
        "document" => "file attachment".to_string(),
        _ => "attachment".to_string(),
    }
}

pub fn presence_key(chat_id: &str) -> String {
    let normalized = normalize_chat_id(chat_id);
    extract_peer_id_from_chat_id(&normalized)
        .map(|peer_id| format!("peer:{peer_id}"))
        .unwrap_or(normalized)
}

fn outgoing_status(chat_id: &str) -> &'static str {
    if chat_id == "Me" {
        "read"
    } else if matches!(
        chat_kind::parse_chat_kind(chat_id),
        ChatKind::Group | ChatKind::TemporaryGroup
    ) {
        "delivered"
    } else {
        "pending"
    }
}

fn next_index(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let next = current as isize + delta;
    next.clamp(0, len.saturating_sub(1) as isize) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchat_core::events::LocalPeerEvent;
    use rchat_core::storage::db::Message;

    fn db_message(chat_id: &str, peer_id: &str, text: &str, id: &str) -> Message {
        Message {
            id: id.to_string(),
            chat_id: chat_id.to_string(),
            peer_id: peer_id.to_string(),
            timestamp: 1_700_000_000,
            content_type: "text".to_string(),
            text_content: Some(text.to_string()),
            file_hash: None,
            status: "delivered".to_string(),
            content_metadata: None,
            sender_alias: None,
        }
    }

    #[test]
    fn local_peer_discovery_updates_peer_list_without_duplicates() {
        let mut state = TuiAppState::default();

        state.apply_local_peer_discovered(LocalPeerEvent {
            peer_id: "peer-1".to_string(),
            addresses: vec!["/ip4/127.0.0.1/tcp/1".to_string()],
        });
        state.apply_local_peer_discovered(LocalPeerEvent {
            peer_id: "peer-1".to_string(),
            addresses: vec!["/ip4/127.0.0.1/tcp/2".to_string()],
        });

        assert_eq!(state.local_peers.len(), 1);
        assert_eq!(state.local_peers[0].peer_id, "peer-1");
        assert_eq!(
            state.local_peers[0].addresses,
            vec!["/ip4/127.0.0.1/tcp/2".to_string()]
        );

        state.apply_local_peer_expired("peer-1");
        assert!(state.local_peers.is_empty());
    }

    #[test]
    fn connected_ids_update_presence_with_self_normalized() {
        let mut state = TuiAppState::default();

        state.apply_connected_chat_ids(vec!["self".to_string(), "peer-1".to_string()]);

        assert!(state.is_chat_connected("Me"));
        assert!(state.is_chat_connected("peer-1"));
    }

    #[test]
    fn selecting_chat_loads_expected_state() {
        let mut state = TuiAppState::default();
        state.replace_chats(vec![TuiChat {
            id: "peer-1".to_string(),
            name: "Peer One".to_string(),
            latest_timestamp: 0,
            unread_count: 3,
        }]);

        state.select_chat_with_history(
            "peer-1",
            vec![db_message("peer-1", "peer-1", "hello", "m1")],
        );

        assert_eq!(state.active_chat_id.as_deref(), Some("peer-1"));
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].text, "hello");
        assert_eq!(state.unread_count("peer-1"), 0);
    }

    #[test]
    fn focus_cycle_skips_local_peers_and_includes_chat_history() {
        let mut state = TuiAppState::default();

        assert_eq!(state.focus, FocusPane::Chats);
        state.cycle_focus();
        assert_eq!(state.focus, FocusPane::History);
        state.cycle_focus();
        assert_eq!(state.focus, FocusPane::ComposerActions);
        state.cycle_focus();
        assert_eq!(state.focus, FocusPane::Composer);
        state.cycle_focus();
        assert_eq!(state.focus, FocusPane::Chats);
    }

    #[test]
    fn composer_action_selection_wraps_with_arrows() {
        let mut state = TuiAppState::default();

        assert_eq!(state.selected_composer_action(), ComposerAction::Attach);
        state.move_composer_action(1);
        assert_eq!(state.selected_composer_action(), ComposerAction::Stickers);
        state.move_composer_action(-1);
        assert_eq!(state.selected_composer_action(), ComposerAction::Attach);
        state.move_composer_action(-1);
        assert_eq!(state.selected_composer_action(), ComposerAction::Details);
    }

    #[test]
    fn sidebar_search_state_collects_and_clears_query() {
        let mut state = TuiAppState::default();

        state.open_sidebar_search();
        state.push_sidebar_search_char('f');
        state.push_sidebar_search_char('e');
        assert_eq!(state.focus, FocusPane::Chats);
        assert!(state.sidebar_search_active);
        assert_eq!(state.sidebar_search, "fe");

        state.pop_sidebar_search_char();
        assert_eq!(state.sidebar_search, "f");
        state.close_sidebar_search();
        assert!(!state.sidebar_search_active);
        assert!(state.sidebar_search.is_empty());
    }

    #[test]
    fn close_modal_clears_help_and_chat_details() {
        let mut state = TuiAppState::default();
        state.show_help = true;
        state.chat_details = Some(TuiChatDetails {
            chat_id: "peer-1".to_string(),
            peer_id: "peer-1".to_string(),
            peer_name: "Peer One".to_string(),
            peer_alias: None,
            avatar_url: None,
            connected: false,
            remote_addr: None,
            reconnect_count: 0,
            sent_total: 0,
            received_total: 0,
            recent_files: Vec::new(),
        });

        state.close_modal();

        assert!(!state.show_help);
        assert!(state.chat_details.is_none());
    }

    #[test]
    fn envelopes_sort_and_assignments_are_lookup_by_normalized_chat_id() {
        let mut state = TuiAppState::default();

        state.replace_envelopes(vec![
            TuiEnvelope {
                id: "z".to_string(),
                name: "Zed".to_string(),
                icon: None,
            },
            TuiEnvelope {
                id: "a".to_string(),
                name: "Archive".to_string(),
                icon: Some("box".to_string()),
            },
        ]);
        state.replace_envelope_assignments(HashMap::from([
            ("peer-1".to_string(), "a".to_string()),
            ("Me".to_string(), "z".to_string()),
        ]));

        assert_eq!(state.envelopes[0].id, "a");
        assert_eq!(state.chat_envelope_id("peer-1"), Some("a"));
        assert_eq!(state.chat_envelope_id("self"), Some("z"));
        assert_eq!(state.chat_envelope_id("peer-2"), None);
    }

    #[test]
    fn chat_history_scrolls_and_resets_on_chat_switch() {
        let mut state = TuiAppState::default();
        state.focus = FocusPane::History;
        state.select_chat_with_history(
            "peer-1",
            vec![
                db_message("peer-1", "peer-1", "one", "m1"),
                db_message("peer-1", "peer-1", "two", "m2"),
                db_message("peer-1", "peer-1", "three", "m3"),
            ],
        );

        state.scroll_history(2);
        assert_eq!(state.history_scroll_offset, 2);

        state.scroll_history(-1);
        assert_eq!(state.history_scroll_offset, 1);

        state.select_chat_with_history("peer-2", Vec::new());
        assert_eq!(state.history_scroll_offset, 0);
    }

    #[test]
    fn history_focus_uses_up_for_older_and_down_for_newer_messages() {
        let mut state = TuiAppState::default();
        state.focus = FocusPane::History;
        state.select_chat_with_history(
            "peer-1",
            vec![
                db_message("peer-1", "peer-1", "one", "m1"),
                db_message("peer-1", "peer-1", "two", "m2"),
                db_message("peer-1", "peer-1", "three", "m3"),
            ],
        );

        assert_eq!(state.selected_message().map(|message| message.id.as_str()), Some("m3"));

        state.move_selection(-1);
        assert_eq!(state.selected_message().map(|message| message.id.as_str()), Some("m2"));
        assert_eq!(state.history_scroll_offset, 1);

        state.move_selection(1);
        assert_eq!(state.selected_message().map(|message| message.id.as_str()), Some("m3"));
        assert_eq!(state.history_scroll_offset, 0);
    }

    #[test]
    fn selected_message_controls_attachment_actions_only_for_attachment_messages() {
        let mut state = TuiAppState::default();
        state.focus = FocusPane::History;
        let mut image = db_message("peer-1", "Me", "image", "image-1");
        image.content_type = "image".to_string();
        image.file_hash = Some("hash-1".to_string());
        state.select_chat_with_history(
            "peer-1",
            vec![db_message("peer-1", "peer-1", "plain", "text-1"), image],
        );

        assert!(state.open_attachment_actions_for_selected());
        state.attachment_actions = None;
        state.move_selection(-1);

        assert_eq!(
            state.selected_message().map(|message| message.id.as_str()),
            Some("text-1")
        );
        assert!(!state.open_attachment_actions_for_selected());
    }

    #[test]
    fn incoming_direct_message_appends_only_for_active_conversation() {
        let mut state = TuiAppState::default();
        state.replace_chats(vec![
            TuiChat {
                id: "peer-1".to_string(),
                name: "Peer One".to_string(),
                latest_timestamp: 0,
                unread_count: 0,
            },
            TuiChat {
                id: "peer-2".to_string(),
                name: "Peer Two".to_string(),
                latest_timestamp: 0,
                unread_count: 0,
            },
        ]);
        state.select_chat_with_history("peer-1", Vec::new());

        let active = state.apply_incoming_message(db_message("peer-1", "peer-1", "active", "m1"));
        let inactive =
            state.apply_incoming_message(db_message("peer-2", "peer-2", "inactive", "m2"));

        assert_eq!(active, IncomingMessageEffect::AppendedToActive);
        assert_eq!(inactive, IncomingMessageEffect::StoredInactive);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].text, "active");
        assert_eq!(state.unread_count("peer-2"), 1);
    }

    #[test]
    fn composer_send_clears_input_only_after_successful_dispatch() {
        let mut state = TuiAppState::default();
        state.select_chat_with_history("peer-1", Vec::new());
        state.composer = " hello ".to_string();

        let draft = state.prepare_composer_send().expect("draft");
        assert_eq!(state.composer, " hello ");

        state.mark_composer_send_failed("network down");
        assert_eq!(state.composer, " hello ");
        assert!(state.messages.is_empty());

        state.mark_composer_send_succeeded(draft, "msg-1".to_string(), 1_700_000_001);
        assert!(state.composer.is_empty());
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].text, "hello");
    }

    #[test]
    fn media_messages_preserve_file_identity_for_rendering() {
        let message = Message {
            id: "m1".to_string(),
            chat_id: "peer-1".to_string(),
            peer_id: "peer-1".to_string(),
            timestamp: 1_700_000_000,
            content_type: "sticker".to_string(),
            text_content: None,
            file_hash: Some("hash-1".to_string()),
            status: "delivered".to_string(),
            content_metadata: Some("{\"size_bytes\":2048}".to_string()),
            sender_alias: None,
        };

        let tui_message = TuiMessage::from(message);

        assert_eq!(tui_message.content_type, "sticker");
        assert_eq!(tui_message.text, "");
        assert_eq!(tui_message.file_hash.as_deref(), Some("hash-1"));
        assert_eq!(
            tui_message.content_metadata.as_deref(),
            Some("{\"size_bytes\":2048}")
        );
    }

    #[test]
    fn attachment_selection_tracks_media_messages_only() {
        let mut state = TuiAppState::default();
        let mut image = db_message("peer-1", "Me", "", "image-1");
        image.content_type = "image".to_string();
        image.file_hash = Some("hash-1".to_string());
        let mut document = db_message("peer-1", "Me", "report.pdf", "doc-1");
        document.content_type = "document".to_string();
        document.file_hash = Some("hash-2".to_string());

        state.select_chat_with_history(
            "peer-1",
            vec![
                db_message("peer-1", "peer-1", "plain", "text-1"),
                image,
                document,
            ],
        );

        assert_eq!(
            state.selected_attachment_message_id.as_deref(),
            Some("doc-1")
        );
        assert_eq!(
            state
                .selected_attachment_message()
                .map(|message| message.id.as_str()),
            Some("doc-1")
        );

        state.move_attachment_selection(-1);
        assert_eq!(
            state
                .selected_attachment_message()
                .map(|message| message.id.as_str()),
            Some("image-1")
        );

        assert!(state.open_attachment_actions_for_selected());
        assert_eq!(
            state
                .attachment_actions
                .as_ref()
                .map(|modal| modal.file_hash.as_str()),
            Some("hash-1")
        );
    }

    #[test]
    fn media_viewer_opens_only_for_attachment_messages() {
        let mut state = TuiAppState::default();
        let mut image = db_message("peer-1", "Me", "", "image-1");
        image.content_type = "image".to_string();
        image.file_hash = Some("hash-1".to_string());

        state.select_chat_with_history(
            "peer-1",
            vec![db_message("peer-1", "peer-1", "plain", "text-1"), image],
        );

        assert!(state.open_media_viewer_for_selected());
        let viewer = state.media_viewer.as_ref().expect("viewer opens");
        assert_eq!(viewer.file_hash, "hash-1");
        assert_eq!(viewer.kind, MediaViewerKind::Image);
        assert!(state.attachment_actions.is_none());

        state.select_chat_with_history(
            "peer-1",
            vec![db_message("peer-1", "peer-1", "plain", "text-1")],
        );
        assert!(!state.open_media_viewer_for_selected());
        assert!(state.media_viewer.is_none());
    }

    #[test]
    fn media_viewer_reset_zoom_and_pan_restores_defaults() {
        let mut viewer = MediaViewerState::new(
            "m1".to_string(),
            "hash-1".to_string(),
            "photo.png".to_string(),
            "image".to_string(),
            None,
        );
        viewer.zoom_in();
        viewer.pan_by(6, 3);

        assert!(viewer.zoom_percent > 100);
        assert_ne!((viewer.pan_x, viewer.pan_y), (0, 0));

        viewer.reset_view();

        assert_eq!(viewer.zoom_percent, 100);
        assert_eq!((viewer.pan_x, viewer.pan_y), (0, 0));
    }

    #[test]
    fn media_viewer_arrows_only_move_zoomed_images() {
        let mut viewer = MediaViewerState::new(
            "m1".to_string(),
            "hash-1".to_string(),
            "photo.png".to_string(),
            "image".to_string(),
            None,
        );

        assert!(!viewer.pan_by(64, 64));

        assert_eq!((viewer.pan_x, viewer.pan_y), (0, 0));

        viewer.zoom_in();
        assert!(viewer.pan_by(64, 64));

        assert_eq!((viewer.pan_x, viewer.pan_y), (64, 64));

        assert!(viewer.pan_by(-128, -128));

        assert_eq!((viewer.pan_x, viewer.pan_y), (-64, -64));

        viewer.zoom_out();

        assert_eq!(viewer.zoom_percent, 100);
        assert_eq!((viewer.pan_x, viewer.pan_y), (0, 0));
    }

    #[test]
    fn attachment_and_sticker_modals_preserve_expected_focus() {
        let mut state = TuiAppState::default();
        state.open_attachment_modal();
        let modal = state.attachment_modal.as_mut().expect("attachment modal");
        assert_eq!(modal.kind, MediaKind::Image);
        assert_eq!(modal.focus, AttachmentModalField::Picker);
        modal.cycle_kind(1);
        assert_eq!(modal.kind, MediaKind::Document);
        modal.push_char('/');
        assert_eq!(modal.picker_query, "/");
        modal.set_picker_entries(vec![
            attachment_file_entry_named("photos", "/tmp/photos", true),
            attachment_file_entry_named("family-photo.png", "/tmp/family-photo.png", false),
            attachment_file_entry_named("report.pdf", "/tmp/report.pdf", false),
        ]);
        modal.picker_query = "fp".to_string();
        assert_eq!(
            modal.selected_entry().map(|entry| entry.name.as_str()),
            Some("family-photo.png")
        );
        assert!(attachment_entry_matches_query("family-photo.png", "fp"));
        modal.set_path(PathBuf::from("/tmp/family-photo.png"));
        assert_eq!(modal.selected_preview_path(), Some("/tmp/family-photo.png"));

        state.open_sticker_picker(vec![
            TuiSticker {
                file_hash: "first".to_string(),
                name: Some("first.webp".to_string()),
                size_bytes: 10,
            },
            TuiSticker {
                file_hash: "second".to_string(),
                name: Some("second.webp".to_string()),
                size_bytes: 20,
            },
        ]);
        let picker = state.sticker_picker.as_mut().expect("sticker picker");
        picker.move_selection(1);
        assert_eq!(picker.selected_hash(), Some("second"));
        assert_eq!(
            picker
                .selected_preview_message()
                .and_then(|message| message.file_hash),
            Some("second".to_string())
        );
        picker.enter_add_path_mode();
        picker.push_char('/');
        picker.push_char('t');
        assert_eq!(picker.mode, StickerPickerMode::AddPath);
        assert_eq!(picker.add_path, "/t");
        picker.pop_char();
        assert_eq!(picker.add_path, "/");
        picker.exit_add_path_mode();
        assert_eq!(picker.mode, StickerPickerMode::Browse);
        assert!(picker.add_path.is_empty());
    }

    #[test]
    fn opening_new_person_starts_at_network_selection() {
        let mut state = TuiAppState::default();

        state.open_new_person();

        let modal = state.new_person.as_ref().expect("modal opens");
        assert_eq!(modal.step, NewPersonStep::SelectNetwork);
        assert_eq!(modal.focus, NewPersonField::LocalNetwork);
        assert!(!state.show_command_palette);
    }

    #[test]
    fn new_person_back_navigation_matches_step_tree() {
        let mut modal = NewPersonModalState::default();

        modal.set_step(NewPersonStep::CreateInviteCode);
        assert!(modal.go_back());
        assert_eq!(modal.step, NewPersonStep::CreateInviteUser);
        assert!(modal.go_back());
        assert_eq!(modal.step, NewPersonStep::Online);
        assert!(modal.go_back());
        assert_eq!(modal.step, NewPersonStep::SelectNetwork);
        assert!(!modal.go_back());

        modal.set_step(NewPersonStep::AcceptInviteCode);
        assert!(modal.go_back());
        assert_eq!(modal.step, NewPersonStep::AcceptInviteUser);

        modal.set_step(NewPersonStep::TemporaryChat);
        assert!(modal.go_back());
        assert_eq!(modal.step, NewPersonStep::Online);
    }

    #[test]
    fn new_person_focus_moves_across_visible_fields() {
        let mut modal = NewPersonModalState::default();

        modal.cycle_focus(0);
        assert_eq!(modal.focus, NewPersonField::OnlineNetwork);
        modal.cycle_focus(0);
        assert_eq!(modal.focus, NewPersonField::OnlineNetwork);
        modal.move_focus(-1, 0);
        assert_eq!(modal.focus, NewPersonField::LocalNetwork);

        modal.set_step(NewPersonStep::LocalScan);
        modal.cycle_focus(2);
        assert_eq!(modal.focus, NewPersonField::LocalPeer(1));
        modal.cycle_focus(2);
        assert_eq!(modal.focus, NewPersonField::LocalPeer(1));
        modal.move_focus(-1, 2);
        assert_eq!(modal.focus, NewPersonField::LocalPeer(0));
    }

    #[test]
    fn new_person_arrows_move_through_visible_fields() {
        let mut modal = NewPersonModalState::default();

        modal.move_focus(1, 0);
        assert_eq!(modal.focus, NewPersonField::OnlineNetwork);
        modal.move_focus(-1, 0);
        assert_eq!(modal.focus, NewPersonField::LocalNetwork);

        modal.set_step(NewPersonStep::AcceptInviteCode);
        modal.move_focus(1, 0);
        assert_eq!(modal.focus, NewPersonField::InviteQrPath);
        modal.move_focus(1, 0);
        assert_eq!(modal.focus, NewPersonField::DecodeInviteQr);
        modal.move_focus(-1, 0);
        assert_eq!(modal.focus, NewPersonField::InviteQrPath);
    }

    #[test]
    fn new_person_text_input_edits_focused_fields() {
        let mut modal = NewPersonModalState::default();
        modal.set_step(NewPersonStep::AcceptInviteCode);

        modal.push_char('a');
        modal.push_char('b');
        assert_eq!(modal.invite_password, "ab");
        modal.pop_char();
        assert_eq!(modal.invite_password, "a");

        modal.focus = NewPersonField::InviteQrPath;
        modal.push_char('/');
        assert_eq!(modal.invite_qr_path, "/");
    }

    #[test]
    fn opening_settings_starts_on_menu_tab() {
        let mut state = TuiAppState::default();

        state.open_settings();

        let modal = state.settings.as_ref().expect("settings opens");
        assert_eq!(modal.section, SettingsSection::Profile);
        assert_eq!(modal.focus, SettingsField::Section(0));
        assert!(state.new_person.is_none());
        assert!(!state.show_command_palette);
    }

    #[test]
    fn settings_focus_and_section_routing_are_predictable() {
        let mut modal = SettingsModalState::default();

        assert_eq!(modal.pane, SettingsPane::Menu);
        modal.cycle_focus();
        assert_eq!(modal.pane, SettingsPane::Content);
        assert_eq!(modal.focus, SettingsField::ProfileAlias);
        modal.cycle_focus();
        assert_eq!(modal.pane, SettingsPane::Menu);
        assert_eq!(modal.focus, SettingsField::Section(0));

        modal.move_section(1);
        assert_eq!(modal.section, SettingsSection::Peers);
        assert_eq!(modal.focus, SettingsField::Section(1));

        modal.move_section(-1);
        assert_eq!(modal.section, SettingsSection::Profile);
    }

    #[test]
    fn activating_theme_section_focuses_theme_controls_immediately() {
        let mut modal = SettingsModalState::default();
        modal.activate_section(SettingsSection::Theme);

        assert_eq!(modal.section, SettingsSection::Theme);
        assert_eq!(modal.pane, SettingsPane::Content);
        assert!(matches!(
            modal.focus,
            SettingsField::ThemePreset(_) | SettingsField::ThemeApply
        ));

        modal.cycle_focus();
        assert_eq!(modal.pane, SettingsPane::Menu);
        assert_eq!(modal.focus, SettingsField::Section(3));
    }

    #[test]
    fn settings_profile_edits_update_draft_fields() {
        let mut modal = SettingsModalState::default();

        modal.focus = SettingsField::ProfileAlias;
        modal.push_char('a');
        modal.push_char('t');
        modal.push_char('a');
        assert_eq!(modal.profile_alias, "ata");
        modal.pop_char();
        assert_eq!(modal.profile_alias, "at");

        modal.focus = SettingsField::ProfileAvatar;
        modal.push_char('/');
        assert_eq!(modal.profile_avatar_path, "/");
    }

    #[test]
    fn settings_connectivity_choice_tracks_expected_mode() {
        let mut modal = SettingsModalState::default();

        modal.connectivity = ConnectivitySettings::from_mode(ConnectivityMode::Lan);
        assert_eq!(modal.connectivity.mode, ConnectivityMode::Lan);

        modal.connectivity = ConnectivitySettings::from_mode(ConnectivityMode::Invisible);
        assert_eq!(modal.connectivity.mode, ConnectivityMode::Invisible);
    }

    #[test]
    fn settings_sticker_path_import_records_input_state() {
        let mut modal = SettingsModalState::default();

        modal.set_section(SettingsSection::Stickers);
        modal.focus = SettingsField::StickerPath;
        for ch in "/tmp/sticker.png".chars() {
            modal.push_char(ch);
        }

        assert_eq!(modal.sticker_path, "/tmp/sticker.png");
    }

    #[test]
    fn settings_sticker_delete_uses_explicit_selection() {
        let mut modal = SettingsModalState::default();
        modal.stickers = vec![
            TuiSticker {
                file_hash: "first".to_string(),
                name: Some("first.webp".to_string()),
                size_bytes: 10,
            },
            TuiSticker {
                file_hash: "second".to_string(),
                name: Some("second.webp".to_string()),
                size_bytes: 20,
            },
        ];

        modal.focus = SettingsField::StickerDelete;
        assert_eq!(modal.selected_sticker_hash(), None);

        modal.select_sticker(1);
        assert_eq!(modal.selected_sticker_hash(), Some("second"));
    }

    #[test]
    fn settings_close_does_not_disturb_active_chat_state() {
        let mut state = TuiAppState::default();
        state.select_chat_with_history("peer-1", Vec::new());

        state.open_settings();
        state.close_settings();

        assert_eq!(state.active_chat_id.as_deref(), Some("peer-1"));
        assert!(state.settings.is_none());
    }
}
