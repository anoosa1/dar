//! Core tables from `next/module/src/index.ts`. All are private: browsers read through views,
//! and the runtime acts through the service identity.

use spacetimedb::{Identity, ScheduleAt, table};

#[table(accessor = instance)]
pub struct Instance {
    #[primary_key]
    pub id: String,
    pub service: Identity,
    pub issuer: String,
}

#[table(accessor = accounts)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    #[primary_key]
    pub id: String,
    #[unique]
    pub username: String,
    pub display_name: String,
    pub verifier: String,
    pub administrator: bool,
    pub enabled: bool,
    pub created_at: f64,
}

#[table(accessor = sessions)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub user_id: String,
    #[unique]
    pub digest: String,
    pub csrf_digest: String,
    pub expires_at: f64,
    pub active: bool,
    pub created_at: f64,
}

/// A browser tab's (or app password's) SpacetimeDB identity and the session it acts for.
#[table(accessor = principals)]
pub struct PrincipalRow {
    #[primary_key]
    pub identity: Identity,
    pub session_id: String,
}

#[table(accessor = session_expiry, scheduled(crate::auth::expire_session))]
pub struct SessionExpiry {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub session_id: String,
}

#[table(accessor = file_entries)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    pub parent_id: String,
    pub name: String,
    pub kind: String,
    pub blob_id: String,
    pub size: f64,
    pub media_type: String,
    pub revision: u32,
    pub modified_at: f64,
    pub trashed: bool,
    pub favorite: bool,
    /// `owner/parent/lowercased name`, or `owner/trash/name/id` in the trash.
    #[index(btree)]
    pub list_key: String,
}

#[table(accessor = versions)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub file_id: String,
    #[index(btree)]
    pub blob_id: String,
    pub size: f64,
    pub media_type: String,
    pub created_at: f64,
}

#[table(accessor = grants)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantRow {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub file_id: String,
    #[index(btree)]
    pub user_id: String,
    pub role: String,
    #[index(btree)]
    #[default("")]
    pub list_key: String,
}

#[table(accessor = uploads)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Upload {
    #[primary_key]
    pub id: String,
    pub session_id: String,
    pub owner_id: String,
    pub parent_id: String,
    pub file_id: String,
    pub name: String,
    pub size: f64,
    pub media_type: String,
    pub expected_revision: u32,
    pub expires_at: f64,
    pub state: String,
    pub digest: String,
    #[index(btree)]
    #[default("")]
    pub expires_key: String,
}

/// `quota` is the storage limit in bytes; 0 means unlimited.
#[table(accessor = usage)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[primary_key]
    pub user_id: String,
    pub used: f64,
    pub reserved: f64,
    #[default(0.0)]
    pub quota: f64,
}

#[table(accessor = favorites)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Favorite {
    #[primary_key]
    pub id: String,
    pub file_id: String,
    #[index(btree)]
    pub list_key: String,
}

/// Stored content by the SHA-256 of its bytes (`module/src/blobs.ts`).
#[table(accessor = file_blobs)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileBlob {
    #[primary_key]
    pub id: String,
    pub size: f64,
    /// "stored" or "deleting" (claimed by maintenance).
    pub state: String,
    /// When maintenance next checks whether anything uses it; "" for never.
    #[index(btree)]
    pub check_key: String,
}

/// SHA-1 and MD5 for WebDAV sync clients that compare only those.
#[table(accessor = blob_checksums)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobChecksum {
    #[primary_key]
    pub id: String,
    pub sha1: String,
    pub md5: String,
}

#[table(accessor = app_passwords)]
#[derive(Clone)]
pub struct AppPassword {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub user_id: String,
    #[unique]
    pub digest: String,
    pub label: String,
    pub scope: String,
    #[unique]
    pub session_id: String,
    pub principal: Identity,
    pub expires_at: f64,
    pub revoked: bool,
}

#[table(accessor = root_revisions)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootRevision {
    #[primary_key]
    pub user_id: String,
    pub revision: u32,
    pub modified_at: f64,
}

#[table(accessor = activity)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    pub file_id: String,
    pub action: String,
    pub at: f64,
}

/// A browser session that confirmed the password may open the Locked folder until `expires_at`.
#[table(accessor = file_unlocks)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileUnlock {
    #[primary_key]
    pub session_id: String,
    #[index(btree)]
    pub user_id: String,
    pub expires_at: f64,
}

/// What a browser tab is looking at in Files; `my_files` shows it (`navigation`).
#[table(accessor = navigation)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Navigation {
    /// The tab's identity, in hex.
    #[primary_key]
    pub session_id: String,
    pub parent_id: String,
    pub section: String,
    pub query: String,
    pub cursor: String,
}

/// A secret download link for one file; only the token's digest is stored.
#[table(accessor = public_shares)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicShare {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub file_id: String,
    #[unique]
    pub digest: String,
    pub expires_at: f64,
}

/// Per-account preferences as JSON (`pim_preferences`); `instance:appearance` holds the logo.
#[table(accessor = pim_preferences)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PimPreference {
    #[primary_key]
    pub user_id: String,
    pub value: String,
    pub revision: u32,
}

#[table(accessor = file_unlock_expiry, scheduled(crate::browse::expire_file_unlock))]
pub struct FileUnlockExpiry {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub session_id: String,
}

/// Address books, calendars and notebooks (`pim-tables.ts`).
#[table(accessor = pim_collections)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PimCollection {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    pub kind: String,
    pub name: String,
    pub color: String,
    pub revision: u32,
    pub modified_at: f64,
}

/// One vCard or iCalendar resource with its parsed projection (JSON).
#[table(accessor = pim_resources, index(accessor = by_collection_href, btree(columns = [collection_id, href])))]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PimResource {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub collection_id: String,
    pub href: String,
    #[unique]
    pub href_key: String,
    pub uid: String,
    #[unique]
    pub uid_key: String,
    pub content: String,
    pub projection: String,
    pub revision: u32,
    pub modified_at: f64,
}

#[table(accessor = pim_grants)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PimGrant {
    /// `collectionId/userId`.
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub collection_id: String,
    #[index(btree)]
    pub user_id: String,
    pub role: String,
}

/// Each account's change counter; pages reload when it moves (`my_pim_changes`).
#[table(accessor = pim_changes)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PimChange {
    #[primary_key]
    pub user_id: String,
    pub revision: u32,
}

#[table(accessor = pim_reminders)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PimReminder {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub user_id: String,
    #[index(btree)]
    pub resource_id: String,
    pub resource_revision: u32,
    pub due_at: f64,
    pub title: String,
    pub acknowledged: bool,
}

/// Where reminder planning for a calendar resource stands (planning is not ported yet).
#[table(accessor = pim_reminder_cursors)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PimReminderCursor {
    #[primary_key]
    pub resource_id: String,
    pub revision: u32,
    pub at: f64,
    #[index(btree)]
    #[default("")]
    pub run_at: String,
}

/// A calendar kept in sync with a remote ICS address (sync is not ported yet).
#[table(accessor = pim_subscriptions)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PimSubscription {
    #[primary_key]
    pub collection_id: String,
    #[index(btree)]
    pub owner_id: String,
    pub url: String,
    pub refresh_minutes: u32,
    #[index(btree)]
    pub next_at: String,
    pub last_sync_at: f64,
    pub last_error: String,
    pub etag: String,
    pub last_modified: String,
}

// ---------------------------------------------------------------------------------------
// The other apps' tables, column for column as `next/module/src/*-tables.ts` define them
// (rust/tools/schema-diff.mjs checks), so a TypeScript database upgrades to this module.

#[table(accessor = backup_pins)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPin {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub backup_id: String,
    #[index(btree)]
    pub blob_id: String,
}

/// Applied Files storage migrations; 1 is content-addressed blobs.
#[table(accessor = file_migrations)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMigration {
    #[primary_key]
    pub id: u32,
    pub applied_at: f64,
}

/// Extracted text of stored note and text files, for search (`text-index.ts`).
#[table(accessor = blob_texts)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobText {
    #[primary_key]
    pub id: String,
    pub text: String,
}

// Identity (identity-tables.ts).
#[table(accessor = directory_profiles)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryProfile {
    #[primary_key]
    pub user_id: String,
    pub document: String,
    pub revision: u32,
    pub modified_at: f64,
    pub deleted: bool,
}

#[table(accessor = directory_groups)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryGroup {
    #[primary_key]
    pub id: String,
    pub document: String,
    pub revision: u32,
    pub created_at: f64,
    pub modified_at: f64,
}

#[table(accessor = directory_members)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryMember {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub group_id: String,
    #[index(btree)]
    pub user_id: String,
}

#[table(accessor = provisioning_tokens)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningToken {
    #[primary_key]
    pub id: String,
    #[unique]
    pub digest: String,
    #[index(btree)]
    pub owner_id: String,
    pub label: String,
    pub scope: String,
    pub expires_at: f64,
    pub revoked: bool,
}

#[table(accessor = identity_proofs)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityProof {
    #[primary_key]
    pub digest: String,
    #[index(btree)]
    pub session_id: String,
    pub verifier: String,
    pub expires_at: f64,
}

#[table(accessor = identity_clients)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityClient {
    #[primary_key]
    pub id: String,
    pub metadata: String,
    pub encrypted: String,
    pub enabled: bool,
    pub revision: u32,
    pub modified_at: f64,
}

#[table(accessor = provider_models)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModel {
    #[primary_key]
    pub id: String,
    pub model: String,
    pub encrypted: String,
    #[index(btree)]
    pub account_id: String,
    #[index(btree)]
    pub client_id: String,
    #[index(btree)]
    pub grant_key: String,
    #[index(btree)]
    pub session_key: String,
    #[index(btree)]
    pub uid_key: String,
    #[index(btree)]
    pub user_code_key: String,
    #[index(btree)]
    pub native_session_id: String,
    pub expires_at: f64,
    #[index(btree)]
    pub expiry_key: String,
    pub consumed: f64,
}

#[table(accessor = identity_migrations)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityMigration {
    #[primary_key]
    pub id: u32,
    pub applied_at: f64,
}

// Workspace (workspace-tables.ts).
/// Trash is private to the notebook owner, even when a writer trashes a shared note.
#[table(accessor = note_trash)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteTrash {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    pub document: String,
    pub notebook_name: String,
    pub trashed_at: f64,
}

#[table(accessor = workspace_migrations)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMigration {
    #[primary_key]
    pub id: u32,
    pub applied_at: f64,
}

// Folder history (history-tables.ts).
#[table(accessor = file_repos)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRepo {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    pub head_id: String,
    pub sequence: u32,
    pub revision: u32,
    pub created_at: f64,
    pub modified_at: f64,
}

#[table(accessor = file_changesets)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeset {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub repo_id: String,
    pub branch: String,
    pub parent_id: String,
    pub sequence: u32,
    pub kind: String,
    pub message: String,
    pub author_id: String,
    pub file_count: u32,
    pub created_at: f64,
}

#[table(accessor = file_changes)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub changeset_id: String,
    pub path: String,
    pub previous_path: String,
    pub kind: String,
    pub file_id: String,
    #[index(btree)]
    pub version_id: String,
    pub previous_version_id: String,
    pub expect_revision: u32,
    #[default("")]
    pub mode: String,
    #[default("")]
    pub previous_mode: String,
}

#[table(accessor = file_trees)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTree {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub repo_id: String,
    pub path: String,
    pub file_id: String,
    pub version_id: String,
    #[default("")]
    pub mode: String,
}

#[table(accessor = file_branches)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileBranch {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub repo_id: String,
    pub name: String,
    pub thread_id: String,
    pub changeset_id: String,
    pub author_id: String,
    pub created_at: f64,
    pub modified_at: f64,
}

#[table(accessor = file_git_objects)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileGitObject {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub repo_id: String,
    pub sha: String,
    pub r#type: String,
    #[index(btree)]
    pub changeset_id: String,
    pub parents: String,
    pub author: String,
    pub raw: String,
    pub created_at: f64,
}

#[table(accessor = file_git_refs)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileGitRef {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub repo_id: String,
    pub name: String,
    pub target: String,
    pub updated_at: f64,
}

#[table(accessor = file_git_blobs)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileGitBlob {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub repo_id: String,
    pub sha: String,
    #[index(btree)]
    pub blob_id: String,
    pub size: f64,
}

// Issues and pull requests (tracker-tables.ts).
#[table(accessor = git_issues)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitIssue {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub repo_id: String,
    pub number: u32,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub state: String,
    pub author_id: String,
    pub created_at: f64,
    pub updated_at: f64,
    pub closed_at: f64,
    pub closed_by: String,
    pub labels: String,
    pub base: String,
    pub head: String,
    pub base_sha: String,
    pub head_sha: String,
    pub merged_sha: String,
    pub revision: u32,
}

#[table(accessor = git_comments)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitComment {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub issue_id: String,
    #[index(btree)]
    pub repo_id: String,
    pub sequence: u32,
    pub kind: String,
    pub author_id: String,
    pub body: String,
    pub data: String,
    pub created_at: f64,
    pub edited_at: f64,
}

#[table(accessor = git_labels)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitLabel {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub repo_id: String,
    pub name: String,
    pub color: String,
    pub description: String,
    pub created_at: f64,
}

// Installed apps (app-tables.ts).
#[table(accessor = installed_apps)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    #[primary_key]
    pub id: String,
    pub document: String,
}

#[table(accessor = app_data)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppData {
    #[primary_key]
    pub id: String,
    pub app_id: String,
    #[index(btree)]
    pub owner_id: String,
    #[index(btree)]
    pub owner_key: String,
    pub key: String,
    pub revision: u32,
    pub schema_version: u32,
    pub value: String,
    pub modified_at: f64,
}

// Agent (agent-tables.ts).
#[table(accessor = agent_projects)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProject {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    pub name: String,
    pub source_type: String,
    pub source_ref: String,
    pub created_at: f64,
}

#[table(accessor = agent_threads)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThread {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    #[index(btree)]
    pub project_id: String,
    pub title: String,
    pub revision: u32,
    pub created_at: f64,
    pub modified_at: f64,
}

#[table(accessor = agent_messages)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessage {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    #[index(btree)]
    pub thread_id: String,
    pub sequence: u32,
    pub role: String,
    pub content: String,
    pub created_at: f64,
}

#[table(accessor = agent_shares)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentShare {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub thread_id: String,
    #[index(btree)]
    pub owner_id: String,
    #[index(btree)]
    pub grantee_id: String,
    pub created_at: f64,
}

#[table(accessor = agent_runs)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRun {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    #[index(btree)]
    pub thread_id: String,
    pub session_id: String,
    pub message_id: String,
    pub model: String,
    pub lease: String,
    pub status: String,
    pub document: String,
    pub created_at: f64,
    pub modified_at: f64,
}

// News (news-tables.ts).
#[table(accessor = news_folders)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsFolder {
    #[primary_key]
    pub id: u32,
    #[index(btree)]
    pub owner_id: String,
    pub name: String,
    pub opened: bool,
    pub created_at: f64,
}

#[table(accessor = news_feeds)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsFeed {
    #[primary_key]
    pub id: u32,
    #[index(btree)]
    pub owner_id: String,
    pub folder_id: u32,
    pub url: String,
    #[unique]
    pub url_key: String,
    pub title: String,
    pub link: String,
    pub added: f64,
    pub ordering: u32,
    pub pinned: bool,
    pub full_text: bool,
    pub paused: bool,
    pub etag: String,
    pub last_modified_header: String,
    pub last_updated: f64,
    pub next_update: f64,
    #[index(btree)]
    pub due_key: String,
    pub error_count: u32,
    pub last_error: String,
}

#[table(accessor = news_items)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsItem {
    #[primary_key]
    pub id: u32,
    pub owner_id: String,
    #[index(btree)]
    pub feed_id: u32,
    #[unique]
    pub dedupe_key: String,
    #[index(btree)]
    pub owner_key: String,
    #[index(btree)]
    pub feed_key: String,
    #[index(btree)]
    pub unread_key: String,
    #[index(btree)]
    pub starred_key: String,
    #[index(btree)]
    pub modified_key: String,
    pub guid: String,
    pub guid_hash: String,
    pub url: String,
    pub title: String,
    pub author: String,
    pub pub_date: f64,
    pub updated_date: f64,
    pub excerpt: String,
    pub enclosure_mime: String,
    pub enclosure_link: String,
    pub media_thumbnail: String,
    pub media_description: String,
    pub rtl: bool,
    pub fingerprint: String,
    pub content_hash: String,
    pub unread: bool,
    pub starred: bool,
    pub last_modified: f64,
    pub position: f64,
    pub fetched_at: f64,
}

#[table(accessor = news_bodies)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsBody {
    #[primary_key]
    pub item_id: u32,
    pub body: String,
}

#[table(accessor = news_counts)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsCount {
    #[primary_key]
    pub owner_id: String,
    pub items: u32,
}

#[table(accessor = news_sequences)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsSequence {
    #[primary_key]
    pub name: String,
    pub value: u32,
}

// Chat (chat-tables.ts).
#[table(accessor = chat_accounts)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAccount {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    pub platform: String,
    pub name: String,
    pub jid: String,
    pub state: String,
    pub detail: String,
    pub created_at: f64,
    pub linked_at: f64,
    pub synced_at: f64,
    pub history_progress: u32,
    pub importing: bool,
    pub messages: u32,
}

#[table(accessor = chat_keys)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatKey {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub scan_key: String,
    #[index(btree)]
    pub expiry_key: String,
    pub value: String,
}

#[table(accessor = chat_threads)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatThread {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub account_id: String,
    #[index(btree)]
    pub list_key: String,
    pub jid: String,
    pub kind: String,
    pub name: String,
    pub at: f64,
    pub preview: String,
    pub preview_kind: String,
    pub preview_from_me: bool,
    pub preview_sender: String,
    pub preview_status: String,
    pub unread: u32,
    pub archived: bool,
    pub pinned: bool,
    pub muted_until: f64,
    #[default("")]
    pub preview_sender_jid: String,
    #[default("")]
    pub server_id: String,
    #[default("")]
    pub section: String,
    #[default("")]
    pub order: String,
    #[default("")]
    pub topic: String,
    #[default(0u32)]
    pub mentions: u32,
}

#[table(accessor = chat_servers)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatServer {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub account_id: String,
    pub server_id: String,
    pub name: String,
    pub order: String,
    pub muted_until: f64,
}

#[table(accessor = chat_messages)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub account_id: String,
    #[index(btree)]
    pub thread_key: String,
    pub message_id: String,
    pub sender: String,
    pub sender_name: String,
    pub from_me: bool,
    pub at: f64,
    pub kind: String,
    pub text: String,
    pub file_name: String,
    pub quoted_id: String,
    pub quoted_sender: String,
    pub quoted_text: String,
    pub status: String,
    pub edited: bool,
    pub deleted: bool,
}

#[table(accessor = chat_media)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMedia {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub account_id: String,
    #[index(btree)]
    pub blob_id: String,
    pub kind: String,
    pub mime: String,
    pub size: f64,
    pub file_name: String,
    pub seconds: u32,
    pub width: u32,
    pub height: u32,
    pub state: String,
    pub at: f64,
}

#[table(accessor = chat_reactions)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReaction {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub message_row_id: String,
    #[index(btree)]
    pub account_id: String,
    pub sender: String,
    pub sender_name: String,
    pub from_me: bool,
    pub emoji: String,
    pub at: f64,
    #[default(1u32)]
    pub count: u32,
}

#[table(accessor = chat_changes)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatChange {
    #[primary_key]
    pub user_id: String,
    pub revision: u32,
}

// Photos and Immich (photos-tables.ts).
#[table(accessor = photo_assets)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoAsset {
    #[primary_key]
    pub file_id: String,
    #[index(btree)]
    pub owner_id: String,
    pub blob_id: String,
    #[index(btree)]
    pub time_key: String,
    pub kind: String,
    pub taken_at: f64,
    pub width: u32,
    pub height: u32,
    pub duration: f64,
    pub exif: String,
    pub has_location: bool,
    pub latitude: f64,
    pub longitude: f64,
    pub favorite: bool,
    pub archived: bool,
    pub description: String,
    pub thumbnail: String,
    pub revision: u32,
    pub indexed_at: f64,
    #[default(1u32)]
    pub index_version: u32,
    #[default("")]
    pub phash: String,
    #[index(btree)]
    #[default("")]
    pub dup_key: String,
    #[default("")]
    pub playback: String,
    #[index(btree)]
    #[default("")]
    pub live_key: String,
    #[index(btree)]
    #[default("")]
    pub stack_id: String,
    #[index(btree)]
    #[default("")]
    pub sha1: String,
    #[default(0.0)]
    pub sync_seq: f64,
    #[index(btree)]
    #[default("")]
    pub sync_key: String,
}

#[table(accessor = photo_albums)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoAlbum {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    pub name: String,
    pub description: String,
    pub cover_id: String,
    pub created_at: f64,
    pub updated_at: f64,
    pub revision: u32,
    #[default(0.0)]
    pub sync_seq: f64,
}

#[table(accessor = photo_album_items)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoAlbumItem {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub album_id: String,
    #[index(btree)]
    pub file_id: String,
    pub added_at: f64,
    #[default(0.0)]
    pub sync_seq: f64,
}

#[table(accessor = photo_state)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoState {
    #[primary_key]
    pub name: String,
    pub value: String,
}

#[table(accessor = photo_stacks)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoStack {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    #[index(btree)]
    pub key: String,
    pub primary_id: String,
    pub created_at: f64,
    pub updated_at: f64,
    #[default(0.0)]
    pub sync_seq: f64,
}

#[table(accessor = photo_album_members)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoAlbumMember {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub album_id: String,
    #[index(btree)]
    pub user_id: String,
    pub role: String,
    pub added_at: f64,
    #[default(0.0)]
    pub sync_seq: f64,
}

#[table(accessor = photo_links)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoLink {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    #[unique]
    pub digest: String,
    pub sealed: String,
    #[index(btree)]
    pub album_id: String,
    pub expires_at: f64,
    pub password_verifier: String,
    pub allow_download: bool,
    pub allow_upload: bool,
    pub show_metadata: bool,
    pub created_at: f64,
    pub revision: u32,
}

#[table(accessor = photo_link_items)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoLinkItem {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub link_id: String,
    #[index(btree)]
    pub file_id: String,
}

#[table(accessor = photo_link_passes)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoLinkPass {
    #[primary_key]
    pub digest: String,
    #[index(btree)]
    pub link_id: String,
    pub expires_at: f64,
}

#[table(accessor = photo_sync_deletes)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoSyncDelete {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub key: String,
    pub kind: String,
    pub entity_id: String,
    pub extra_id: String,
    pub audience: String,
    pub seq: f64,
    pub at: f64,
}

#[table(accessor = immich_sessions)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImmichSession {
    #[primary_key]
    pub digest: String,
    #[unique]
    pub id: String,
    #[index(btree)]
    pub user_id: String,
    #[index(btree)]
    pub session_id: String,
    pub device_type: String,
    #[serde(rename = "deviceOS")]
    pub device_os: String,
    pub created_at: f64,
    pub used_at: f64,
    pub epoch_seen: f64,
}

#[table(accessor = immich_checkpoints)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImmichCheckpoint {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub immich_id: String,
    pub r#type: String,
    pub ack: String,
}

#[table(accessor = immich_connections)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImmichConnection {
    #[primary_key]
    pub user_id: String,
    pub url: String,
    pub sealed_key: String,
    pub direction: String,
    pub interval_minutes: u32,
    pub enabled: bool,
    pub last_run_at: f64,
    pub last_error: String,
    pub summary: String,
    pub revision: u32,
}

#[table(accessor = immich_remote)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImmichRemote {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub user_id: String,
    pub remote_id: String,
    #[index(btree)]
    pub file_id: String,
    pub checksum: String,
}

// Books (books-tables.ts).
#[table(accessor = book_states)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookState {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    #[index(btree)]
    pub file_id: String,
    pub status: String,
    pub position: String,
    pub progress: f64,
    pub device: String,
    pub updated_at: f64,
    pub revision: u32,
}

#[table(accessor = book_marks)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookMark {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    #[index(btree)]
    pub key: String,
    pub file_id: String,
    pub position: String,
    pub label: String,
    pub created_at: f64,
}

// Mail (mail.rs). Providers come from the administrator (Nix or Administration); each account's
// messages are mirrored as .eml files in Files, which these rows index.
#[table(accessor = mail_providers)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailProvider {
    #[primary_key]
    pub id: String,
    pub name: String,
    /// "nix" (the host configuration, replaced at every start) or "admin" (Administration).
    pub source: String,
    /// Servers, sign-in kind and OAuth endpoints as JSON; never a secret.
    pub document: String,
    /// The OAuth client secret sealed by the runtime (Administration providers only).
    pub secret: String,
    pub enabled: bool,
    pub ordering: u32,
    pub modified_at: f64,
}

#[table(accessor = mail_accounts)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailAccount {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub owner_id: String,
    #[index(btree)]
    pub provider_id: String,
    pub name: String,
    pub address: String,
    pub display_name: String,
    pub username: String,
    /// The password, token or OAuth grant, sealed by the runtime.
    pub credential: String,
    pub color: u32,
    pub ordering: u32,
    /// The account's Files folder (`docs/mail/<name>`).
    #[index(btree)]
    pub folder_id: String,
    pub status: String,
    pub last_error: String,
    pub error_count: u32,
    pub synced_at: f64,
    pub next_sync: f64,
    #[index(btree)]
    pub due_key: String,
    pub signature: String,
    pub created_at: f64,
}

#[table(accessor = mail_mailboxes)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailMailbox {
    #[primary_key]
    pub id: String,
    #[index(btree)]
    pub account_id: String,
    pub owner_id: String,
    /// The IMAP mailbox name or the JMAP mailbox ID.
    pub remote: String,
    pub name: String,
    pub role: String,
    pub parent_id: String,
    #[index(btree)]
    pub folder_id: String,
    /// IMAP UIDVALIDITY; a change means every UID was reassigned.
    pub validity: String,
    pub total: u32,
    pub unread: u32,
}

#[table(accessor = mail_messages)]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailMessage {
    #[primary_key]
    pub id: String,
    pub owner_id: String,
    #[index(btree)]
    pub account_id: String,
    #[index(btree)]
    pub mailbox_id: String,
    /// The IMAP UID or the JMAP email ID; empty while a message added in Files is on its way.
    pub remote: String,
    #[unique]
    pub remote_key: String,
    #[index(btree)]
    pub file_id: String,
    pub blob_id: String,
    pub message_id: String,
    #[index(btree)]
    pub thread_key: String,
    pub subject: String,
    pub from_name: String,
    pub from_address: String,
    /// `{to, cc}` as JSON lists of `{name, address}`.
    pub recipients: String,
    pub date: f64,
    pub snippet: String,
    pub seen: bool,
    pub flagged: bool,
    pub answered: bool,
    pub draft: bool,
    pub attachments: bool,
    pub size: f64,
    /// `<owner>/<newest first>/<id>` and the same per mailbox, role, unread and flagged.
    #[index(btree)]
    pub owner_key: String,
    #[index(btree)]
    pub mailbox_key: String,
    #[index(btree)]
    pub role_key: String,
    #[index(btree)]
    pub unread_key: String,
    #[index(btree)]
    pub flagged_key: String,
}
