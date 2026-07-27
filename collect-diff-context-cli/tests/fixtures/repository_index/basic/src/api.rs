use crate::{
    auth::{validate_token as validate, Validator},
    nested::inner::nested_validate,
};

pub fn login(token: &str) -> bool {
    validate(token) && Validator::validate(token) && nested_validate(token)
}

pub fn default_allowed() -> bool {
    crate::auth::DEFAULT_ALLOWED
}
