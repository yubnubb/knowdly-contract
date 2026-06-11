# knowdly-contract

Soroban smart contract for the Knowdly digital publishing platform. Handles book registration, USDC payment splitting, NFT ownership minting, resale marketplace, and on-chain royalty enforcement.

---

## Deployment

| Network | Contract address |
|---------|-----------------|
| Stellar testnet | `CBSEVXLOG72CW6L77J6HGRSMKI3IXKLKQTPPMLQS65NNHGGNXGOXMXAA` |
| Stellar mainnet | Pending SCF milestone 3 |

---

## Overview

Knowdly allows creators to publish encrypted digital books permanently to Arweave. This contract manages the on-chain layer: book registration, purchase, NFT minting, and resale with enforced creator royalties.

Every payment split is atomic. The contract enforces creator royalties on every transfer. Royalty rates are immutable after registration.

---

## Contract functions

### Initialisation

```rust
initialise(admin: Address) -> Result<(), Error>
```
Sets the contract admin. Called once at deployment.

```rust
upgrade(new_wasm_hash: BytesN<32>) -> Result<(), Error>
```
Upgrades the contract WASM. Admin only.

---

### Book management

```rust
register_book(
    creator: Address,
    arweave_tx_id: String,
    price: i128,
    royalty_rate: u32
) -> Result<u64, Error>
```
Registers a new book. Stores the Arweave transaction ID, price in USDC stroops, creator address, and royalty rate (basis points). Returns the assigned book ID. Royalty rate is immutable after registration.

```rust
deactivate_book(book_id: u64) -> Result<(), Error>
```
Deactivates a book. Creator or admin only. Does not affect existing ownership tokens.

```rust
update_arweave_tx(book_id: u64, new_tx_id: String) -> Result<(), Error>
```
Updates the Arweave transaction ID for a book. Used if content is re-uploaded. Creator only.

---

### Purchase

```rust
purchase(book_id: u64, buyer: Address) -> Result<u64, Error>
```
Processes a book purchase. Atomically splits the USDC payment:
- 97.5% to the creator
- 2.5% to the platform treasury

Mints an ownership NFT to the buyer's wallet. Returns the token ID.

---

### Ownership

```rust
owns_book(book_id: u64, wallet: Address) -> bool
```
Returns true if the wallet holds a valid ownership token for the book. Called by the key server before releasing the AES decryption key.

```rust
transfer_token(token_id: u64, to: Address) -> Result<(), Error>
```
Transfers an ownership token. Enforces creator royalty on transfer if a resale listing exists.

---

### Read functions

```rust
get_book(book_id: u64) -> Result<Book, Error>
get_token(token_id: u64) -> Result<Token, Error>
get_total_books() -> u64
get_total_tokens() -> u64
get_tokens_by_owner(wallet: Address) -> Vec<u64>
```

---

### Resale marketplace

```rust
list_for_sale(token_id: u64, price: i128) -> Result<u64, Error>
```
Lists an owned token for resale at the specified USDC price. Returns the listing ID.

```rust
buy_listing(listing_id: u64, buyer: Address) -> Result<(), Error>
```
Purchases a resale listing. Atomically splits the USDC payment:
- Creator royalty (set at book registration, enforced by contract)
- Seller receives remainder minus platform fee
- 2.5% to platform treasury

Transfers the NFT to the buyer. Invalidates the seller's decryption access.

```rust
cancel_listing(listing_id: u64) -> Result<(), Error>
```
Cancels an active listing. Seller only.

```rust
get_listing(listing_id: u64) -> Result<Listing, Error>
```
Returns listing details.

---

## Data structures

```rust
pub struct Book {
    pub id: u64,
    pub creator: Address,
    pub arweave_tx_id: String,
    pub price: i128,
    pub royalty_rate: u32,   // basis points e.g. 500 = 5%
    pub active: bool,
}

pub struct Token {
    pub id: u64,
    pub book_id: u64,
    pub owner: Address,
}

pub struct Listing {
    pub id: u64,
    pub token_id: u64,
    pub seller: Address,
    pub price: i128,
    pub active: bool,
}
```

---

## Payment splits

### Primary sale

| Recipient | Share |
|-----------|-------|
| Creator | 97.5% |
| Platform treasury | 2.5% |

### Resale

| Recipient | Share |
|-----------|-------|
| Creator royalty | Set at registration (e.g. 5%) |
| Platform treasury | 2.5% |
| Seller | Remainder |

All splits are enforced atomically by the contract. No off-chain settlement.

---

## Build and test

```bash
# Build
cargo build --target wasm32-unknown-unknown --release

# Run tests
cargo test

# Deploy to testnet
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/knowdly_contract.wasm \
  --network testnet \
  --source <your-key>
```

---

## Dependencies

```toml
[dependencies]
soroban-sdk = { version = "21.0.0", features = ["testutils"] }
```

---

## Licence

This repository contains the Knowdly smart contract only. The broader platform — frontend, key server, encryption implementation — is proprietary and not included here.

---

## Links

- Platform: [knowdly.com](https://knowdly.com)
- SCF submission: SCF #44
- Stellar testnet explorer: [stellar.expert/explorer/testnet/contract/CBSEVXLOG72CW6L77J6HGRSMKI3IXKLKQTPPMLQS65NNHGGNXGOXMXAA](https://stellar.expert/explorer/testnet/contract/CBSEVXLOG72CW6L77J6HGRSMKI3IXKLKQTPPMLQS65NNHGGNXGOXMXAA)
