use fixture::{api::login, exported_validate};

#[test]
fn accepts_token() {
    assert!(login("token"));
    assert!(exported_validate("token"));
}

