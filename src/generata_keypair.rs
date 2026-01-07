use std::io::Error;

use sui_sdk::types::{
    base_types::SuiAddress,
    crypto::{self, SuiKeyPair},
};

pub fn create_keypaire() -> Result<(SuiAddress, String), Error> {
    let kp = crypto::get_account_key_pair(); // generating a new keypair;
    /*
     * kp.0 = newly generated address
     * SuiKeyPair::Ed25519(kp.1).encode().unwrap() = private key in this format suiprivkey...
     */
    Ok((kp.0, SuiKeyPair::Ed25519(kp.1).encode().unwrap()))
}
