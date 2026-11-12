//! Work-target confirmation for the reception: unconfirmed → proposed → confirmed.
//!
//! The path comes only from explicit user input; nothing here consults env vars,
//! the home or current directory. Confirming only validates; a confirmed target
//! yields a [`WorkSessionPlan`] data value and never starts a session.

use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    ReadOnly,
    ReadWrite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkTargetState {
    Unconfirmed,
    Proposed,
    Confirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkTargetError {
    Empty,
    NotAbsolute,
    Missing,
    NotADirectory,
    Unreadable,
    NoPermission,
    NotConfirmed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkSessionPlan {
    pub working_dir: PathBuf,
    pub permission: Permission,
}

#[derive(Debug, Default)]
pub struct WorkTarget {
    input: String,
    permission: Option<Permission>,
    confirmed: Option<PathBuf>,
}

impl WorkTarget {
    /// Stores the user's raw input. A different value drops any confirmation.
    pub fn set_path(&mut self, input: &str) {
        if self.input != input {
            self.input = input.to_owned();
            self.confirmed = None;
        }
    }

    /// Records the explicit choice. A different value drops any confirmation.
    pub fn set_permission(&mut self, permission: Permission) {
        if self.permission != Some(permission) {
            self.permission = Some(permission);
            self.confirmed = None;
        }
    }

    pub fn path_input(&self) -> &str {
        &self.input
    }

    pub fn permission(&self) -> Option<Permission> {
        self.permission
    }

    pub fn confirmed_dir(&self) -> Option<&Path> {
        self.confirmed.as_deref()
    }

    pub fn state(&self) -> WorkTargetState {
        if self.confirmed.is_some() {
            WorkTargetState::Confirmed
        } else if self.input.trim().is_empty() && self.permission.is_none() {
            WorkTargetState::Unconfirmed
        } else {
            WorkTargetState::Proposed
        }
    }

    /// Validates the input and, on success, stores its canonical path.
    pub fn confirm(&mut self) -> Result<&Path, WorkTargetError> {
        self.confirmed = None;
        let canonical = validate(&self.input)?;
        if self.permission.is_none() {
            return Err(WorkTargetError::NoPermission);
        }
        Ok(self.confirmed.insert(canonical))
    }

    /// Re-validates the confirmed directory and returns the plan; never starts a session.
    pub fn plan(&mut self) -> Result<WorkSessionPlan, WorkTargetError> {
        let (Some(dir), Some(permission)) = (&self.confirmed, self.permission) else {
            return Err(WorkTargetError::NotConfirmed);
        };
        if let Err(error) = validate_dir(dir) {
            self.confirmed = None;
            return Err(error);
        }
        Ok(WorkSessionPlan {
            working_dir: dir.clone(),
            permission,
        })
    }
}

fn validate(input: &str) -> Result<PathBuf, WorkTargetError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(WorkTargetError::Empty);
    }
    let path = Path::new(trimmed);
    // Relative input would resolve against the current dir, which must not be inferred.
    if !path.is_absolute() {
        return Err(WorkTargetError::NotAbsolute);
    }
    validate_dir(path)?;
    std::fs::canonicalize(path).map_err(|_| WorkTargetError::Unreadable)
}

fn validate_dir(path: &Path) -> Result<(), WorkTargetError> {
    let metadata = std::fs::metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => WorkTargetError::Missing,
        _ => WorkTargetError::Unreadable,
    })?;
    if !metadata.is_dir() {
        return Err(WorkTargetError::NotADirectory);
    }
    std::fs::read_dir(path).map_err(|_| WorkTargetError::Unreadable)?;
    Ok(())
}

impl Permission {
    pub fn label(self) -> &'static str {
        match self {
            Permission::ReadOnly => "読み取り専用",
            Permission::ReadWrite => "読み書き",
        }
    }
}

impl WorkTargetState {
    pub fn label(self) -> &'static str {
        match self {
            WorkTargetState::Unconfirmed => "未確認",
            WorkTargetState::Proposed => "確認待ち",
            WorkTargetState::Confirmed => "確定済み",
        }
    }
}

impl fmt::Display for WorkTargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            WorkTargetError::Empty => "作業先のパスを入力してください",
            WorkTargetError::NotAbsolute => "作業先は絶対パスで入力してください",
            WorkTargetError::Missing => "作業先が見つかりません",
            WorkTargetError::NotADirectory => "作業先がフォルダではありません",
            WorkTargetError::Unreadable => "作業先を読み取れません",
            WorkTargetError::NoPermission => "権限（読み取り専用 / 読み書き）を選んでください",
            WorkTargetError::NotConfirmed => "作業先が確定していません",
        })
    }
}

impl std::error::Error for WorkTargetError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "h2o-work-target-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn input(path: &Path) -> String {
        path.to_str().unwrap().to_owned()
    }

    fn confirmed(name: &str, permission: Permission) -> (PathBuf, WorkTarget) {
        let dir = fixture(name);
        let mut target = WorkTarget::default();
        target.set_path(&input(&dir));
        target.set_permission(permission);
        target.confirm().unwrap();
        (dir, target)
    }

    #[test]
    fn starts_unconfirmed_without_path_or_permission() {
        let target = WorkTarget::default();
        assert_eq!(target.state(), WorkTargetState::Unconfirmed);
        assert_eq!(target.path_input(), "");
        assert_eq!(target.permission(), None);
        assert_eq!(target.confirmed_dir(), None);
    }

    #[test]
    fn empty_and_whitespace_input_rejected() {
        for raw in ["", "   ", "\t\n"] {
            let mut target = WorkTarget::default();
            target.set_path(raw);
            target.set_permission(Permission::ReadOnly);
            assert_eq!(target.confirm(), Err(WorkTargetError::Empty), "{raw:?}");
            assert_ne!(target.state(), WorkTargetState::Confirmed);
        }
    }

    #[test]
    fn relative_input_rejected_instead_of_resolving_against_current_dir() {
        let mut target = WorkTarget::default();
        target.set_path(".");
        target.set_permission(Permission::ReadOnly);
        assert_eq!(target.confirm(), Err(WorkTargetError::NotAbsolute));
        assert_ne!(target.state(), WorkTargetState::Confirmed);
    }

    #[test]
    fn missing_dir_rejected_and_not_created() {
        let missing = fixture("missing").join("does-not-exist");
        let mut target = WorkTarget::default();
        target.set_path(&input(&missing));
        target.set_permission(Permission::ReadWrite);
        assert_eq!(target.confirm(), Err(WorkTargetError::Missing));
        assert!(!missing.exists());
        assert_eq!(target.state(), WorkTargetState::Proposed);
    }

    #[test]
    fn file_rejected_as_not_a_directory() {
        let file = fixture("file").join("plain.txt");
        fs::write(&file, "x").unwrap();
        let mut target = WorkTarget::default();
        target.set_path(&input(&file));
        target.set_permission(Permission::ReadOnly);
        assert_eq!(target.confirm(), Err(WorkTargetError::NotADirectory));
        assert_eq!(target.state(), WorkTargetState::Proposed);
    }

    #[test]
    fn confirmed_value_is_canonical_path() {
        let dir = fixture("canonical");
        fs::create_dir(dir.join("sub")).unwrap();
        let canonical = fs::canonicalize(&dir).unwrap();
        for raw in [
            input(&dir.join("sub").join("..")),
            input(&dir.join(".")),
            format!("  {}  ", input(&dir)),
        ] {
            let mut target = WorkTarget::default();
            target.set_path(&raw);
            target.set_permission(Permission::ReadOnly);
            assert_eq!(target.confirm(), Ok(canonical.as_path()), "{raw:?}");
            assert_eq!(target.state(), WorkTargetState::Confirmed);
            assert_eq!(target.confirmed_dir(), Some(canonical.as_path()));
        }
    }

    #[test]
    fn no_default_permission_confirm_requires_choice() {
        let dir = fixture("no-permission");
        let mut target = WorkTarget::default();
        target.set_path(&input(&dir));
        assert_eq!(target.permission(), None);
        assert_eq!(target.state(), WorkTargetState::Proposed);
        assert_eq!(target.confirm(), Err(WorkTargetError::NoPermission));
        assert_eq!(target.state(), WorkTargetState::Proposed);
        target.set_permission(Permission::ReadOnly);
        assert!(target.confirm().is_ok());
    }

    #[test]
    fn changing_path_after_confirm_drops_to_proposed() {
        let (dir, mut target) = confirmed("path-change", Permission::ReadOnly);
        target.set_path(&input(&dir.join(".")));
        assert_eq!(target.state(), WorkTargetState::Proposed);
        assert_eq!(target.confirmed_dir(), None);
        assert_eq!(target.plan(), Err(WorkTargetError::NotConfirmed));
    }

    #[test]
    fn changing_permission_after_confirm_drops_to_proposed() {
        let (_dir, mut target) = confirmed("permission-change", Permission::ReadOnly);
        target.set_permission(Permission::ReadWrite);
        assert_eq!(target.state(), WorkTargetState::Proposed);
        assert_eq!(target.plan(), Err(WorkTargetError::NotConfirmed));
    }

    // Documented choice: re-setting the identical value is not a change, so a UI
    // that writes the field back every frame does not silently drop confirmation.
    #[test]
    fn setting_same_value_keeps_confirmation() {
        let (dir, mut target) = confirmed("same-value", Permission::ReadWrite);
        target.set_path(&input(&dir));
        target.set_permission(Permission::ReadWrite);
        assert_eq!(target.state(), WorkTargetState::Confirmed);
    }

    #[test]
    fn plan_refused_when_unconfirmed_or_proposed() {
        let mut target = WorkTarget::default();
        assert_eq!(target.plan(), Err(WorkTargetError::NotConfirmed));
        target.set_path(&input(&fixture("plan-proposed")));
        target.set_permission(Permission::ReadOnly);
        assert_eq!(target.state(), WorkTargetState::Proposed);
        assert_eq!(target.plan(), Err(WorkTargetError::NotConfirmed));
    }

    #[test]
    fn plan_contains_canonical_dir_and_permission() {
        let (dir, mut target) = confirmed("plan-ok", Permission::ReadWrite);
        let plan = target.plan().unwrap();
        assert_eq!(
            plan,
            WorkSessionPlan {
                working_dir: fs::canonicalize(&dir).unwrap(),
                permission: Permission::ReadWrite,
            }
        );
        assert_eq!(target.state(), WorkTargetState::Confirmed);
    }

    #[test]
    fn plan_revalidates_and_drops_confirmation_when_dir_gone() {
        let (dir, mut target) = confirmed("plan-gone", Permission::ReadOnly);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(target.plan(), Err(WorkTargetError::Missing));
        assert_eq!(target.state(), WorkTargetState::Proposed);
        assert_eq!(target.confirmed_dir(), None);
        assert_eq!(target.plan(), Err(WorkTargetError::NotConfirmed));
    }

    #[test]
    fn japanese_labels_and_messages() {
        assert_eq!(WorkTargetState::Unconfirmed.label(), "未確認");
        assert_eq!(WorkTargetState::Proposed.label(), "確認待ち");
        assert_eq!(WorkTargetState::Confirmed.label(), "確定済み");
        assert_eq!(Permission::ReadOnly.label(), "読み取り専用");
        assert_eq!(Permission::ReadWrite.label(), "読み書き");
        let errors = [
            WorkTargetError::Empty,
            WorkTargetError::NotAbsolute,
            WorkTargetError::Missing,
            WorkTargetError::NotADirectory,
            WorkTargetError::Unreadable,
            WorkTargetError::NoPermission,
            WorkTargetError::NotConfirmed,
        ];
        let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
        for (error, message) in errors.iter().zip(&messages) {
            assert!(!message.is_empty(), "{error:?}");
        }
        let mut unique = messages.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), messages.len(), "messages must be distinct");
    }
}
