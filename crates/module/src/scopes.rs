//! App-password scopes (`next/shared/app-scopes.ts`).

pub const PASSWORD_APPS: [&str; 5] = ["files", "contacts", "calendar", "news", "photos"];

/// Legacy write scopes included read access; new space-separated (`v2 …`) grants are independent.
pub fn password_grants(scope: &str) -> Vec<String> {
    match scope {
        "read" => return vec!["files:read".into()],
        "write" => return vec!["files:read".into(), "files:write".into()],
        _ => {}
    }
    if let Some((app, permission)) = scope.split_once(':')
        && matches!(app, "contacts" | "calendar" | "pim")
        && matches!(permission, "read" | "write")
    {
        let apps: &[&str] = if app == "pim" {
            &["contacts", "calendar"]
        } else {
            &[app]
        };
        return apps
            .iter()
            .flat_map(|a| {
                if permission == "write" {
                    vec![format!("{a}:read"), format!("{a}:write")]
                } else {
                    vec![format!("{a}:read")]
                }
            })
            .collect();
    }
    scope
        .strip_prefix("v2 ")
        .map(|rest| rest.split(' ').map(str::to_owned).collect())
        .unwrap_or_default()
}

pub fn valid_password_scope(scope: &str) -> bool {
    let grants = password_grants(scope);
    let unique: std::collections::BTreeSet<&String> = grants.iter().collect();
    !grants.is_empty()
        && grants.len() <= 10
        && unique.len() == grants.len()
        && grants.iter().all(|g| {
            PASSWORD_APPS
                .iter()
                .any(|app| *g == format!("{app}:read") || *g == format!("{app}:write"))
        })
}

pub fn password_allows(scope: &str, app: &str, permission: &str) -> bool {
    password_grants(scope)
        .iter()
        .any(|g| *g == format!("{app}:{permission}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_v2_scopes() {
        assert!(password_allows("write", "files", "read"));
        assert!(!password_allows("read", "files", "write"));
        assert!(password_allows("pim:write", "calendar", "read"));
        assert!(password_allows("v2 files:write", "files", "write"));
        assert!(
            !password_allows("v2 files:write", "files", "read"),
            "v2 grants are independent"
        );
        assert!(valid_password_scope("v2 files:read contacts:write"));
        assert!(!valid_password_scope("v2 files:read files:read"));
        assert!(!valid_password_scope("v2 mail:read"));
    }
}
