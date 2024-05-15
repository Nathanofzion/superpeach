#![cfg(test)]

use soroban_sdk::{testutils::BytesN as _, BytesN, Env};

mod factory {
    soroban_sdk::contractimport!(file = "../out/webauthn_factory.optimized.wasm");
}

mod passkey {
    use soroban_sdk::auth::Context;

    soroban_sdk::contractimport!(file = "../out/webauthn_account_secp256r1.optimized.wasm");
}

#[test]
fn test() {
    let env = Env::default();

    env.mock_all_auths();

    let factory_address = env.register_contract_wasm(None, factory::WASM);
    let factory_client = factory::Client::new(&env, &factory_address);

    let passkkey_hash = env.deployer().upload_contract_wasm(passkey::WASM);

    factory_client.init(&passkkey_hash);

    let salt: BytesN<32> = BytesN::random(&env);
    let pk: BytesN<65> = BytesN::random(&env);

    let deployee_address = factory_client.deploy(&salt, &pk);
    let deployee_client = passkey::Client::new(&env, &deployee_address);

    deployee_client.add_sig(&salt, &pk);
}
