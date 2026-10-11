//! Completed preparation records outlive their application units.

use super::*;

impl Runtime {
    pub fn remove_app_until(&self, prepared: &AppRecord, deadline: &Deadline) -> Result<(), SessionError> {
        let _lease = self.app_lease_until(&prepared.unit, deadline)?;
        let current = match self.app(&prepared.unit) {
            Ok(app) => app,
            Err(SessionError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if current.invocation_id != prepared.invocation_id {
            return Err(SessionError::State("application invocation changed before record retirement".into()));
        }
        self.remove_app(&prepared.unit)
    }

    pub fn archive_app(&self, app: &AppRecord) -> Result<(), SessionError> {
        let file = self.app_path(&app.unit)?;
        let directory = self.path.join("results");
        match fs::create_dir(&directory) {
            Ok(()) => {
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
                sync_directory(&directory)?;
                sync_directory(&self.path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error.into()),
        }
        verify_directory(&directory)?;
        write_record(&directory.join(file.file_name().expect("app record has a filename")), app)
    }

    pub fn archived_app(&self, unit: &str) -> Result<AppRecord, SessionError> {
        let file = self.app_path(unit)?;
        let directory = self.path.join("results");
        verify_directory(&directory)?;
        let record: AppRecord = read_record(&directory.join(file.file_name().expect("app record has a filename")))?;
        if record.unit != unit { return Err(SessionError::State("archived application identity mismatch".into())); }
        record.policy.validate().map_err(|error| SessionError::State(error.into()))?;
        Ok(record)
    }
}
