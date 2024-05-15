#![no_std]

use soroban_sdk::{
    auth::{Context, CustomAccountInterface},
    contract, contracterror, contractimpl, contracttype,
    crypto::Hash,
    symbol_short, Bytes, BytesN, Env, Symbol, Vec,
};

mod base64_url;

mod test;

#[contract]
pub struct Contract;

#[contracterror]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Error {
    NotFound = 1,
    NotPermitted = 2,
    ClientDataJsonChallengeIncorrect = 3,
    Secp256r1PublicKeyParse = 4,
    Secp256r1SignatureParse = 5,
    Secp256r1VerifyFailed = 6,
    JsonParseError = 7,
    InvalidContext = 8,
}

const SIGNERS: Symbol = symbol_short!("sigs");
const SUDO_SIGNER: Symbol = symbol_short!("sudo_sig");

#[contractimpl]
impl Contract {
    pub fn extend_ttl(env: Env) {
        let max_ttl = env.storage().max_ttl();
        let contract_address = env.current_contract_address();

        env.storage().instance().extend_ttl(max_ttl, max_ttl);
        env.deployer()
            .extend_ttl(contract_address.clone(), max_ttl, max_ttl);
        env.deployer()
            .extend_ttl_for_code(contract_address.clone(), max_ttl, max_ttl);
        env.deployer()
            .extend_ttl_for_contract_instance(contract_address.clone(), max_ttl, max_ttl);
    }
    pub fn resudo(env: Env, id: BytesN<32>) -> Result<(), Error> {
        env.current_contract_address().require_auth();

        let sigs = env
            .storage()
            .instance()
            .get::<Symbol, Vec<BytesN<32>>>(&SIGNERS)
            .ok_or(Error::NotFound)?;

        // Ensure the new proposed sudo signer exists
        if sigs.contains(&id) {
            env.storage().instance().set(&SUDO_SIGNER, &id);
        } else {
            return Err(Error::NotFound);
        }

        Self::extend_ttl(env);

        Ok(())
    }
    pub fn rm_sig(env: Env, id: BytesN<32>) -> Result<(), Error> {
        // Don't delete the sudo signer
        if id
            == env
                .storage()
                .instance()
                .get::<Symbol, BytesN<32>>(&SUDO_SIGNER)
                .ok_or(Error::NotFound)?
        {
            return Err(Error::NotPermitted);
        }

        env.current_contract_address().require_auth();

        let mut sigs = env
            .storage()
            .instance()
            .get::<Symbol, Vec<BytesN<32>>>(&SIGNERS)
            .ok_or(Error::NotFound)?;

        match sigs.binary_search(&id) {
            Ok(i) => {
                sigs.remove(i);
                env.storage().instance().set(&SIGNERS, &sigs);
                env.storage().persistent().remove(&id);
            }
            Err(_) => return Err(Error::NotFound),
        };

        Self::extend_ttl(env);

        Ok(())
    }
    pub fn add_sig(env: Env, id: BytesN<32>, pk: BytesN<65>) -> Result<(), Error> {
        if env.storage().instance().has(&SUDO_SIGNER) {
            env.current_contract_address().require_auth();
        }
        // initialize the passkey account with a sudo signer
        else {
            env.storage().instance().set(&SUDO_SIGNER, &id);
        }

        let max_ttl = env.storage().max_ttl();

        env.storage().persistent().set(&id, &pk);
        env.storage().persistent().extend_ttl(&id, max_ttl, max_ttl);

        let mut sigs = env
            .storage()
            .instance()
            .get::<Symbol, Vec<BytesN<32>>>(&SIGNERS)
            .unwrap_or(Vec::new(&env));

        match sigs.binary_search(&id) {
            Ok(_) => return Err(Error::NotPermitted), // don't allow dupes
            Err(i) => {
                sigs.insert(i, id);
                env.storage().instance().set(&SIGNERS, &sigs);
            }
        };

        Self::extend_ttl(env);

        Ok(())
    }
    pub fn list_sigs(env: Env) -> Vec<BytesN<32>> {
        env.storage()
            .instance()
            .get::<Symbol, Vec<BytesN<32>>>(&SIGNERS)
            .unwrap_or(Vec::new(&env))
    }
}

#[contracttype]
pub struct Signature {
    pub id: BytesN<32>,
    pub authenticator_data: Bytes,
    pub client_data_json: Bytes,
    pub signature: BytesN<64>,
}

#[derive(serde::Deserialize)]
struct ClientDataJson<'a> {
    challenge: &'a str,
}

#[contractimpl]
impl CustomAccountInterface for Contract {
    type Error = Error;
    type Signature = Signature;

    #[allow(non_snake_case)]
    fn __check_auth(
        env: Env,
        signature_payload: Hash<32>,
        signature: Signature,
        auth_contexts: Vec<Context>,
    ) -> Result<(), Error> {
        // Only the sudo signer can `add_sig`, `rm_sig` and `resudo`
        for context in auth_contexts.iter() {
            match context {
                Context::Contract(c) => {
                    if c.contract == env.current_contract_address()
                        && (c.fn_name == Symbol::new(&env, "add_sig")
                            || c.fn_name == Symbol::new(&env, "rm_sig")
                            || c.fn_name == Symbol::new(&env, "resudo"))
                    {
                        if signature.id
                            != env
                                .storage()
                                .instance()
                                .get::<Symbol, BytesN<32>>(&SUDO_SIGNER)
                                .ok_or(Error::NotFound)?
                        {
                            return Err(Error::NotPermitted);
                        }
                    }
                }
                Context::CreateContractHostFn(_) => return Err(Error::InvalidContext),
            };
        }

        // Verify that the public key produced the signature.
        let pk = env
            .storage()
            .persistent()
            .get(&signature.id)
            .ok_or(Error::NotFound)?;

        let mut payload = Bytes::new(&env);

        payload.append(&signature.authenticator_data);
        payload.extend_from_array(&env.crypto().sha256(&signature.client_data_json).to_array());
        let payload = env.crypto().sha256(&payload);

        env.crypto()
            .secp256r1_verify(&pk, &payload, &signature.signature);

        // Parse the client data JSON, extracting the base64 url encoded
        // challenge.
        let client_data_json = signature.client_data_json.to_buffer::<1024>();
        let client_data_json = client_data_json.as_slice();
        let (client_data, _): (ClientDataJson, _) =
            serde_json_core::de::from_slice(client_data_json).map_err(|_| Error::JsonParseError)?;

        // Build what the base64 url challenge is expected.
        let mut expected_challenge = *b"___________________________________________";
        base64_url::encode(&mut expected_challenge, &signature_payload.to_array());

        // Check that the challenge inside the client data JSON that was signed
        // is identical to the expected challenge.
        if client_data.challenge.as_bytes() != expected_challenge {
            return Err(Error::ClientDataJsonChallengeIncorrect);
        }

        let max_ttl = env.storage().max_ttl();
        env.storage()
            .persistent()
            .extend_ttl(&signature.id, max_ttl, max_ttl);

        Self::extend_ttl(env);

        Ok(())
    }
}
