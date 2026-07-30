fn main() {
    let key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    println!("{}", chess_core::account::export(&key));
}
