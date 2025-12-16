use rvault_core::crypto::{hash_data, verify_password};
use rvault_core::vault::Vault;
use std::fs;

fn main() {
    let password = "test_password";
    println!("Testing password: '{}'", password);

    // 1. Hash
    match hash_data(password.as_bytes()) {
        Ok(hashed) => {
            println!("Hash generated: {}", hashed.hash);

            // 2. Verify
            let is_valid = verify_password(password.as_bytes(), &hashed.hash);
            println!("Verification result: {}", is_valid);

            if !is_valid {
                println!("CRITICAL: Verification failed immediately after hashing!");
            }
            
            // 3. Get Encryption Key (Expect Failure on CI/Headless if no vault, but check error)
            match Vault::get_encryption_key(password, &hashed.hash) {
                Ok(_) => println!("get_encryption_key: SUCCESS"),
                Err(e) => println!("get_encryption_key: FAILED (Expected if no keystore): {}", e),
            }

        },
        Err(e) => println!("Hashing failed: {}", e),
    }
}
