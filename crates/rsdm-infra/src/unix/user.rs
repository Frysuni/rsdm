use rsdm_core::ports::{ResolvedUser, UserResolveError, UserResolver};

use super::group::user_in_any_group;
use super::nss;

#[derive(Debug, Clone, Default)]
pub struct UnixUserResolver {
    pub deny_root: bool,
    pub allowed_groups: Vec<String>,
}

impl UnixUserResolver {
    pub fn with_allowed_groups(deny_root: bool, allowed_groups: Vec<String>) -> Self {
        Self {
            deny_root,
            allowed_groups,
        }
    }
}

impl UserResolver for UnixUserResolver {
    fn canonical_username(&self, username: &str) -> Result<String, UserResolveError> {
        Ok(lookup_user(username)?.username)
    }

    fn resolve_user(&self, username: &str) -> Result<ResolvedUser, UserResolveError> {
        let user = lookup_user(username)?;
        if self.deny_root && user.uid == 0 {
            return Err(UserResolveError::Denied("root login is denied".to_string()));
        }
        if !self.allowed_groups.is_empty()
            && !user_in_any_group(&user.username, user.gid, &self.allowed_groups)?
        {
            return Err(UserResolveError::Denied(format!(
                "{} is not in an allowed login group",
                user.username
            )));
        }
        Ok(user)
    }
}

fn lookup_user(username: &str) -> Result<ResolvedUser, UserResolveError> {
    let user = nss::user_by_name(username)
        .map_err(|error| UserResolveError::Backend(error.to_string()))?
        .ok_or_else(|| UserResolveError::NotFound(username.to_string()))?;
    Ok(ResolvedUser {
        username: user.username,
        uid: user.uid,
        gid: user.gid,
        // An account may have blank passwd fields; login(1) substitutes / and
        // /bin/sh rather than exporting empty HOME/SHELL into the session.
        home: default_if_empty(user.home, "/"),
        shell: default_if_empty(user.shell, "/bin/sh"),
    })
}

pub fn current_username() -> Option<String> {
    // SAFETY: geteuid never fails and touches no memory.
    let uid = unsafe { libc::geteuid() };
    nss::user_name_by_uid(uid).ok().flatten()
}

pub fn uid_for_username(username: &str) -> Result<Option<u32>, String> {
    nss::uid_by_name(username).map_err(|error| error.to_string())
}

fn default_if_empty(value: String, default: &str) -> String {
    if value.trim().is_empty() {
        default.to_string()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::{default_if_empty, current_username};

    #[test]
    fn blank_passwd_fields_fall_back_to_login_defaults() {
        assert_eq!(default_if_empty(String::new(), "/bin/sh"), "/bin/sh");
        assert_eq!(default_if_empty("  ".to_string(), "/"), "/");
        assert_eq!(
            default_if_empty("/home/alice".to_string(), "/"),
            "/home/alice"
        );
    }

    #[test]
    fn resolves_the_effective_uid() {
        assert!(current_username().is_some());
    }
}
