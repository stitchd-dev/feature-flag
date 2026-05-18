use argon2::{
    Argon2, Params,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};
fn main() {
    let salt = SaltString::generate(&mut OsRng);
    let params = Params::new(19456, 2, 1, None).unwrap();
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let hash = argon2.hash_password(b"password123", &salt).unwrap();
    println!("{}", hash);
}
