use sonora::{
    platform::{Account, Paths, ServerCredentials},
    server::Server,
};

fn account() -> Account {
    Account {
        url: "https://music.example.com/".into(),
        username: "listener".into(),
    }
}

#[test]
fn saved_login_survives_restart_without_storing_the_password() {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths {
        data: directory.path().into(),
        cache: directory.path().join("cache"),
    };
    let account = account();
    assert!(paths.credentials(&account).unwrap().is_none());
    let server = Server::new(account.clone(), "example-password".into()).unwrap();
    paths.save_account(&account).unwrap();
    paths
        .save_credentials(&account, server.credentials())
        .unwrap();
    let reopened = paths.clone();
    let restored_account = reopened.account().unwrap().unwrap();
    let restored = Server::with_credentials(
        restored_account,
        reopened.credentials(&account).unwrap().unwrap(),
    )
    .unwrap();
    let before = server.url("ping", &[]).unwrap();
    let after = restored.url("ping", &[]).unwrap();
    assert!(
        before == after,
        "Restored login must authenticate identically"
    );
    let credentials_path = directory
        .path()
        .join("credentials")
        .join(format!("{}.json", account.namespace()));
    let saved = std::fs::read_to_string(&credentials_path).unwrap();
    assert!(!saved.contains("example-password"));
    assert!(!saved.contains("password"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&credentials_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(credentials_path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    for other in [
        Account {
            username: "other".into(),
            ..account.clone()
        },
        Account {
            url: "https://other.example.com/".into(),
            ..account.clone()
        },
    ] {
        assert!(reopened.credentials(&other).unwrap().is_none());
    }
    paths
        .save_credentials(
            &account,
            &ServerCredentials::from_password("replacement-password").unwrap(),
        )
        .unwrap();
    let replaced = Server::with_credentials(
        account.clone(),
        paths.credentials(&account).unwrap().unwrap(),
    )
    .unwrap();
    assert!(
        before != replaced.url("ping", &[]).unwrap(),
        "Signing in again replaces the saved login"
    );
}

#[test]
fn corrupt_saved_login_requests_sign_in_without_exposing_file_contents() {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths {
        data: directory.path().into(),
        cache: directory.path().join("cache"),
    };
    let account = account();
    paths
        .save_credentials(
            &account,
            &ServerCredentials::from_password("example-password").unwrap(),
        )
        .unwrap();
    let file = directory
        .path()
        .join("credentials")
        .join(format!("{}.json", account.namespace()));
    for contents in [
        "private-token-invalid-json",
        r#"{"salt":"private-salt","token":"private-token"}"#,
    ] {
        std::fs::write(&file, contents).unwrap();
        let message = paths.credentials(&account).err().unwrap().to_string();
        assert!(message.contains("Sign in again"));
        assert!(!message.contains("private"));
    }
    assert!(ServerCredentials::from_password("").is_err());
}

#[test]
fn an_empty_password_reuses_the_saved_login() {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths {
        data: directory.path().into(),
        cache: directory.path().join("cache"),
    };
    let account = account();
    let first = Server::new(account.clone(), "example-password".into()).unwrap();
    paths
        .save_credentials(&first.account, first.credentials())
        .unwrap();
    let again = Server::sign_in(&paths, account.clone(), "").unwrap();
    assert_eq!(
        first.url("ping", &[]).unwrap(),
        again.url("ping", &[]).unwrap()
    );
    let mut unnormalized = account.clone();
    unnormalized.url = "https://music.example.com".into();
    let reused = Server::sign_in(&paths, unnormalized, "").unwrap();
    assert_eq!(
        first.url("ping", &[]).unwrap(),
        reused.url("ping", &[]).unwrap()
    );
    let other = Account {
        url: "https://other.example.com/".into(),
        ..account
    };
    assert!(Server::sign_in(&paths, other, "").is_err());
}

#[test]
fn slskd_key_is_saved_privately_and_rejected_when_damaged() {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths {
        data: directory.path().into(),
        cache: directory.path().join("cache"),
    };
    use sonora::slskd::SlskdAuth;
    // Nothing saved is not an error: slskd is then contacted without a login.
    assert!(matches!(paths.slskd_auth().unwrap(), SlskdAuth::None));
    let key = SlskdAuth::from_fields("  0123456789abcdef-secret\n", "ignored", "ignored")
        .unwrap()
        .unwrap();
    paths.save_slskd_auth(&key).unwrap();
    let path = directory.path().join("credentials").join("slskd.key");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"key":"0123456789abcdef-secret"}"#
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(matches!(paths.slskd_auth().unwrap(), SlskdAuth::Key(_)));
    let login = SlskdAuth::from_fields("", " admin ", "pass word")
        .unwrap()
        .unwrap();
    paths.save_slskd_auth(&login).unwrap();
    assert!(matches!(
        paths.slskd_auth().unwrap(),
        SlskdAuth::Login { username, password } if username == "admin" && password == "pass word"
    ));
    // Nothing typed changes nothing; half a login is refused.
    assert!(SlskdAuth::from_fields(" ", "", "").unwrap().is_none());
    assert!(SlskdAuth::from_fields("", "admin", "").is_err());
    // A bare key written by an earlier version still reads.
    std::fs::write(&path, "0123456789abcdef-secret").unwrap();
    assert!(matches!(paths.slskd_auth().unwrap(), SlskdAuth::Key(_)));
    std::fs::write(&path, "short").unwrap();
    let error = paths.slskd_auth().err().unwrap().to_string();
    assert_eq!(error, "The saved slskd login is invalid. Enter it again.");
    assert!(!error.contains("short"));
}
