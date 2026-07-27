pub mod api;
pub mod auth;

pub use auth::validate_token as exported_validate;

pub mod nested {
    pub mod inner {
        use super::super::auth::validate_token;

        pub fn nested_validate(token: &str) -> bool {
            validate_token(token)
        }
    }

    pub fn via_self(token: &str) -> bool {
        self::inner::nested_validate(token)
    }
}

