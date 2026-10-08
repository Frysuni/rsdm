use rsdm_core::ports::UserResolveError;

use super::nss;

pub fn user_in_any_group(
    username: &str,
    primary_gid: u32,
    allowed_groups: &[String],
) -> Result<bool, UserResolveError> {
    let allowed_gids = allowed_groups
        .iter()
        .map(|name| lookup_group_gid(name))
        .collect::<Result<Vec<_>, _>>()?;

    if allowed_gids.contains(&primary_gid) {
        return Ok(true);
    }

    let user_gids = user_groups(username, primary_gid)?;
    Ok(user_gids.iter().any(|gid| allowed_gids.contains(gid)))
}

fn lookup_group_gid(name: &str) -> Result<u32, UserResolveError> {
    nss::group_gid(name)
        .map_err(|error| UserResolveError::Backend(error.to_string()))?
        .ok_or_else(|| UserResolveError::Denied(format!("allowed group does not exist: {name}")))
}

fn user_groups(username: &str, primary_gid: u32) -> Result<Vec<u32>, UserResolveError> {
    nss::groups_for_user(username, primary_gid)
        .map_err(|error| UserResolveError::Backend(error.to_string()))
}
