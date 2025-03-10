pub mod config;
pub mod kv_store;
pub mod types;
pub mod util;

use anyhow::{anyhow, Result};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

use config::PrivyConfig;
use types::{
    CreateWalletRequest, CreateWalletResponse, PrivyClaims,
    SignAndSendTransactionParams, SignAndSendTransactionRequest,
    SignAndSendTransactionResponse, User,
};

use util::{create_http_client, transaction_to_base64};

pub struct WalletManager {
    privy_config: PrivyConfig,
    http_client: reqwest::Client,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct UserSession {
    pub(crate) user_id: String,
    pub(crate) session_id: String,
    pub(crate) wallet_address: String,
    pub(crate) pubkey: String,
}

/// WalletManager currently only supports Solana
impl WalletManager {
    pub fn new(privy_config: PrivyConfig) -> Self {
        let http_client = create_http_client(&privy_config);
        Self {
            privy_config,
            http_client,
        }
    }

    pub async fn create_wallet(&self) -> Result<CreateWalletResponse> {
        let request = CreateWalletRequest {
            chain_type: "solana".to_string(),
        };

        let response = self
            .http_client
            .post("https://api.privy.io/v1/wallets")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to create wallet: {} - {}",
                response.status(),
                response.text().await?
            ));
        }

        Ok(response.json().await?)
    }

    pub async fn authenticate_user(
        &self,
        access_token: &str,
    ) -> Result<UserSession> {
        let claims = self.validate_access_token(access_token)?;
        let user = self.get_user_by_id(&claims.user_id).await?;
        let solana_wallet = user
            .linked_accounts
            .iter()
            .find_map(|account| match account {
                types::LinkedAccount::Wallet(wallet) => {
                    if wallet.delegated
                        && wallet.chain_type == "solana"
                        && wallet.wallet_client == "privy"
                    {
                        Some(wallet)
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .ok_or_else(|| {
                anyhow!("Could not find a delegated solana wallet")
            })?;
        let evm_wallet = user
            .linked_accounts
            .iter()
            .find_map(|account| match account {
                types::LinkedAccount::Wallet(wallet) => {
                    if wallet.delegated
                        && wallet.chain_type == "ethereum"
                        && wallet.wallet_client == "privy"
                    {
                        Some(wallet)
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .ok_or_else(|| {
                anyhow!("Could not find a delegated ethereum wallet")
            })?;

        Ok(UserSession {
            user_id: user.id,
            session_id: claims.session_id,
            wallet_address: evm_wallet.address.clone(),
            pubkey: solana_wallet.address.clone(),
        })
    }

    #[cfg(feature = "evm")]
    pub async fn sign_and_send_evm_transaction(
        &self,
        address: String,
        transaction: alloy::rpc::types::TransactionRequest,
    ) -> Result<String> {
        use self::types::{
            SignAndSendEvmTransactionParams, SignAndSendEvmTransactionRequest,
        };

        let request = SignAndSendEvmTransactionRequest {
            address,
            chain_type: "ethereum".to_string(),
            method: "eth_sendTransaction".to_string(),
            caip2: "eip155:42161".to_string(), // TODO parametrize this
            params: SignAndSendEvmTransactionParams {
                transaction: serde_json::to_value(transaction)?,
            },
        };

        let response = self
            .http_client
            .post("https://auth.privy.io/api/v1/wallets/rpc")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to send transaction: {}",
                response.text().await?
            ));
        }

        let result: SignAndSendTransactionResponse = response.json().await?;
        Ok(result.data.hash)
    }

    #[cfg(feature = "solana")]
    pub async fn sign_and_send_solana_transaction(
        &self,
        address: String,
        transaction: &solana_sdk::transaction::Transaction,
    ) -> Result<String> {
        let request = SignAndSendTransactionRequest {
            address,
            chain_type: "solana".to_string(),
            method: "signAndSendTransaction".to_string(),
            caip2: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            params: SignAndSendTransactionParams {
                transaction: transaction_to_base64(transaction)?,
                encoding: "base64".to_string(),
            },
        };

        let response = self
            .http_client
            .post("https://api.privy.io/v1/wallets/rpc")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to sign transaction: {}",
                response.text().await?
            ));
        }

        let result: SignAndSendTransactionResponse = response.json().await?;
        Ok(result.data.hash)
    }

    pub fn validate_access_token(
        &self,
        access_token: &str,
    ) -> Result<PrivyClaims> {
        let mut validation = Validation::new(Algorithm::ES256);
        validation.set_issuer(&["privy.io"]);
        validation.set_audience(&[self.privy_config.app_id.clone()]);

        let key = DecodingKey::from_ec_pem(
            self.privy_config.verification_key.as_bytes(),
        )?;

        let token_data =
            decode::<PrivyClaims>(access_token, &key, &validation)
                .map_err(|_| anyhow!("Failed to authenticate"))?;

        Ok(token_data.claims)
    }

    pub async fn get_user_by_id(&self, user_id: &str) -> Result<User> {
        let url = format!("https://auth.privy.io/api/v1/users/{}", user_id);

        let response = self.http_client.get(url).send().await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to get user data: {}",
                response.status()
            ));
        }
        let text = response.text().await?;
        // dbg!(serde_json::from_str::<serde_json::Value>(&text)?);
        Ok(serde_json::from_str(&text)?)
    }
}
