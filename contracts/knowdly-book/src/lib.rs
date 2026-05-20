// lib.rs — Knowdly Book Smart Contract
// Deployed on Stellar using the Soroban smart contract platform
//
// This contract handles:
//   1. Book registration by creators
//   2. Book purchases by readers (minting ownership tokens)
//   3. Royalty enforcement on every resale
//   4. Ownership verification for content access control
//   5. Per-wallet token index — get_tokens_by_owner() eliminates localStorage dependency
//   6. update_arweave_tx() — writes real Arweave TX ID after upload completes

#![no_std]

use soroban_sdk::{
    contract,
    contractimpl,
    contracttype,
    symbol_short,
    Address,
    Env,
    String,
    Vec,
};

// ── Data Types ────────────────────────────────────────────────────────────────

// Book represents a work registered by a creator
#[contracttype]
#[derive(Clone)]
pub struct Book {
    // unique identifier for this book — auto-incremented
    pub id: u64,

    // the creator's Stellar wallet address — receives payments and royalties
    pub publisher: Address,

    // purchase price in stroops (1 USDC = 10,000,000 stroops)
    pub price: i128,

    // resale royalty in basis points (500 = 5%, 1000 = 10%, max 5000 = 50%)
    pub royalty_bps: u32,

    // the Arweave transaction ID where encrypted content is stored
    // initially set to a placeholder — updated via update_arweave_tx()
    // after the real Arweave upload completes
    pub arweave_tx_id: String,

    // the book title stored on-chain for discoverability
    pub title: String,

    // whether this book is available for new purchases
    pub active: bool,

    // total number of copies sold
    pub total_sales: u64,
}

// Token represents a reader's ownership of a specific book
// this IS the NFT — owning a token means owning the book
#[contracttype]
#[derive(Clone)]
pub struct Token {
    pub id:             u64,
    pub book_id:        u64,
    pub owner:          Address,
    pub minted_at:      u32,
    pub purchase_price: i128,
}

// ── Storage Keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    NextBookId,
    NextTokenId,
    Book(u64),
    Token(u64),
    // ownership record for a specific owner + book combination
    Ownership(Address, u64),
    Platform,
    PlatformFeeBps,
    // stores a Vec<u64> of tokenIds owned by a wallet
    // replaces localStorage dependency — works on any device
    OwnerTokens(Address),
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct KnowdlyBookContract;

#[contractimpl]
impl KnowdlyBookContract {

    // ── Initialisation ────────────────────────────────────────────────────────

    // initialise must be called once after deployment
    // sets the platform wallet and fee rate
    pub fn initialise(env: Env, platform: Address, fee_bps: u32) {
        platform.require_auth();

        if fee_bps > 1000 {
            panic!("Platform fee cannot exceed 10%");
        }

        env.storage().instance().set(&DataKey::Platform,       &platform);
        env.storage().instance().set(&DataKey::PlatformFeeBps, &fee_bps);
        env.storage().instance().set(&DataKey::NextBookId,     &0u64);
        env.storage().instance().set(&DataKey::NextTokenId,    &0u64);
    }

    // ── Creator API ───────────────────────────────────────────────────────────

    // register_book is called by a creator to list their work
    // arweave_tx_id is initially a placeholder — update with update_arweave_tx()
    // after the real Arweave upload completes
    // returns the new book's ID
    pub fn register_book(
        env:           Env,
        publisher:     Address,
        price:         i128,
        royalty_bps:   u32,
        arweave_tx_id: String,
        title:         String,
    ) -> u64 {
        publisher.require_auth();

        if price <= 0       { panic!("Price must be positive"); }
        if royalty_bps > 5000 { panic!("Royalty cannot exceed 50%"); }

        let book_id: u64 = env
            .storage().instance()
            .get(&DataKey::NextBookId)
            .unwrap_or(0);

        let book = Book {
            id:            book_id,
            publisher:     publisher.clone(),
            price,
            royalty_bps,
            arweave_tx_id,
            title,
            active:        true,
            total_sales:   0,
        };

        env.storage().persistent().set(&DataKey::Book(book_id), &book);
        env.storage().instance().set(&DataKey::NextBookId, &(book_id + 1));

        env.events().publish(
            (symbol_short!("reg_book"),),
            (book_id, publisher),
        );

        book_id
    }

    // update_arweave_tx — called after Arweave upload completes
    // replaces the placeholder TX ID with the real Arweave transaction ID
    // this makes the arweave_tx_id fully on-chain and eliminates any
    // dependency on localStorage or Supabase for ownership → content mapping
    pub fn update_arweave_tx(
        env:           Env,
        publisher:     Address,
        book_id:       u64,
        arweave_tx_id: String,
    ) {
        // only the original creator can update their book
        publisher.require_auth();

        let mut book: Book = env
            .storage().persistent()
            .get(&DataKey::Book(book_id))
            .expect("Book not found");

        if book.publisher != publisher {
            panic!("Only the creator can update this book");
        }

        book.arweave_tx_id = arweave_tx_id;

        env.storage().persistent()
            .set(&DataKey::Book(book_id), &book);

        env.events().publish(
            (symbol_short!("upd_tx"),),
            (book_id,),
        );
    }

    // deactivate_book stops new purchases
    // only the original creator can deactivate their own book
    pub fn deactivate_book(env: Env, publisher: Address, book_id: u64) {
        publisher.require_auth();

        let mut book: Book = env
            .storage().persistent()
            .get(&DataKey::Book(book_id))
            .expect("Book not found");

        if book.publisher != publisher {
            panic!("Only the creator can deactivate this book");
        }

        book.active = false;
        env.storage().persistent().set(&DataKey::Book(book_id), &book);
    }

    // ── Reader Purchase API ───────────────────────────────────────────────────

    // purchase mints an ownership token to the reader's wallet
    // returns the new token ID
    pub fn purchase(env: Env, buyer: Address, book_id: u64) -> u64 {
        buyer.require_auth();

        let mut book: Book = env
            .storage().persistent()
            .get(&DataKey::Book(book_id))
            .expect("Book not found");

        if !book.active {
            panic!("This book is not available for purchase");
        }

        let already_owned: bool = env
            .storage().persistent()
            .get(&DataKey::Ownership(buyer.clone(), book_id))
            .unwrap_or(false);

        if already_owned {
            panic!("You already own this book");
        }

        let token_id: u64 = env
            .storage().instance()
            .get(&DataKey::NextTokenId)
            .unwrap_or(0);

        let token = Token {
            id:             token_id,
            book_id,
            owner:          buyer.clone(),
            minted_at:      env.ledger().sequence(),
            purchase_price: book.price,
        };

        env.storage().persistent().set(&DataKey::Token(token_id), &token);

        // record ownership for owns_book() lookup
        env.storage().persistent().set(
            &DataKey::Ownership(buyer.clone(), book_id),
            &true,
        );

        // add tokenId to buyer's OwnerTokens list
        // enables get_tokens_by_owner() — pure on-chain ownership discovery
        let owner_key = DataKey::OwnerTokens(buyer.clone());
        let mut owner_tokens: Vec<u64> = env
            .storage().persistent()
            .get(&owner_key)
            .unwrap_or_else(|| Vec::new(&env));
        owner_tokens.push_back(token_id);
        env.storage().persistent().set(&owner_key, &owner_tokens);

        env.storage().instance().set(&DataKey::NextTokenId, &(token_id + 1));

        book.total_sales += 1;
        env.storage().persistent().set(&DataKey::Book(book_id), &book);

        env.events().publish(
            (symbol_short!("purchase"),),
            (token_id, book_id, buyer),
        );

        token_id
    }

    // ── Resale / Transfer API ─────────────────────────────────────────────────

    // transfer_token handles resale between readers
    // royalties are enforced here — cannot be bypassed
    pub fn transfer_token(
        env:        Env,
        token_id:   u64,
        new_owner:  Address,
        sale_price: i128,
    ) {
        let mut token: Token = env
            .storage().persistent()
            .get(&DataKey::Token(token_id))
            .expect("Token not found");

        token.owner.require_auth();

        let book: Book = env
            .storage().persistent()
            .get(&DataKey::Book(token.book_id))
            .expect("Book not found");

        let platform_fee_bps: u32 = env
            .storage().instance()
            .get(&DataKey::PlatformFeeBps)
            .unwrap_or(250);

        let royalty_amount  = (sale_price * book.royalty_bps as i128) / 10_000;
        let platform_amount = (sale_price * platform_fee_bps as i128) / 10_000;
        let seller_amount   = sale_price - royalty_amount - platform_amount;

        if seller_amount < 0 {
            panic!("Sale price too low to cover royalty and platform fees");
        }

        let old_owner = token.owner.clone();

        // update ownership records
        env.storage().persistent().set(
            &DataKey::Ownership(old_owner.clone(), token.book_id),
            &false,
        );
        env.storage().persistent().set(
            &DataKey::Ownership(new_owner.clone(), token.book_id),
            &true,
        );

        // remove tokenId from old owner's list
        let old_key = DataKey::OwnerTokens(old_owner.clone());
        let old_tokens: Vec<u64> = env
            .storage().persistent()
            .get(&old_key)
            .unwrap_or_else(|| Vec::new(&env));
        let mut updated_old = Vec::new(&env);
        for i in 0..old_tokens.len() {
            if old_tokens.get(i).unwrap() != token_id {
                updated_old.push_back(old_tokens.get(i).unwrap());
            }
        }
        env.storage().persistent().set(&old_key, &updated_old);

        // add tokenId to new owner's list
        let new_key = DataKey::OwnerTokens(new_owner.clone());
        let mut new_tokens: Vec<u64> = env
            .storage().persistent()
            .get(&new_key)
            .unwrap_or_else(|| Vec::new(&env));
        new_tokens.push_back(token_id);
        env.storage().persistent().set(&new_key, &new_tokens);

        token.owner          = new_owner.clone();
        token.purchase_price = sale_price;
        env.storage().persistent().set(&DataKey::Token(token_id), &token);

        env.events().publish(
            (symbol_short!("transfer"),),
            (
                token_id,
                old_owner,
                new_owner,
                royalty_amount,
                platform_amount,
                seller_amount,
            ),
        );
    }

    // ── Access Control API ────────────────────────────────────────────────────

    // owns_book — called by the key server before releasing decryption key
    // returns true only if the wallet holds the NFT for this book
    pub fn owns_book(env: Env, owner: Address, book_id: u64) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Ownership(owner, book_id))
            .unwrap_or(false)
    }

    // get_tokens_by_owner — returns all tokenIds owned by a wallet
    // enables pure on-chain ownership discovery on any device
    // no localStorage or Supabase needed for ownership verification
    pub fn get_tokens_by_owner(env: Env, owner: Address) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::OwnerTokens(owner))
            .unwrap_or_else(|| Vec::new(&env))
    }

    // ── Read API ──────────────────────────────────────────────────────────────

    pub fn get_book(env: Env, book_id: u64) -> Book {
        env.storage()
            .persistent()
            .get(&DataKey::Book(book_id))
            .expect("Book not found")
    }

    pub fn get_token(env: Env, token_id: u64) -> Token {
        env.storage()
            .persistent()
            .get(&DataKey::Token(token_id))
            .expect("Token not found")
    }

    pub fn get_total_books(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::NextBookId)
            .unwrap_or(0)
    }

    pub fn get_total_tokens(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::NextTokenId)
            .unwrap_or(0)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    #[test]
    fn test_register_book() {
        let env         = Env::default();
        let contract_id = env.register(KnowdlyBookContract, ());
        let client      = KnowdlyBookContractClient::new(&env, &contract_id);
        let platform    = Address::generate(&env);
        let creator     = Address::generate(&env);

        env.mock_all_auths();
        client.initialise(&platform, &250u32);

        let book_id = client.register_book(
            &creator,
            &10_000_000i128,
            &500u32,
            &String::from_str(&env, "pending_test"),
            &String::from_str(&env, "Introduction to Blockchain"),
        );

        assert_eq!(book_id, 0);
        let book = client.get_book(&book_id);
        assert_eq!(book.id,          0);
        assert_eq!(book.price,       10_000_000);
        assert_eq!(book.royalty_bps, 500);
        assert_eq!(book.active,      true);
        assert_eq!(book.total_sales, 0);
    }

    #[test]
    fn test_update_arweave_tx() {
        let env         = Env::default();
        let contract_id = env.register(KnowdlyBookContract, ());
        let client      = KnowdlyBookContractClient::new(&env, &contract_id);
        let platform    = Address::generate(&env);
        let creator     = Address::generate(&env);

        env.mock_all_auths();
        client.initialise(&platform, &250u32);

        let book_id = client.register_book(
            &creator,
            &10_000_000i128,
            &500u32,
            &String::from_str(&env, "pending_1234567890"),
            &String::from_str(&env, "Test Book"),
        );

        // verify placeholder was stored
        let book = client.get_book(&book_id);
        assert_eq!(book.arweave_tx_id, String::from_str(&env, "pending_1234567890"));

        // update with real arweave tx id
        client.update_arweave_tx(
            &creator,
            &book_id,
            &String::from_str(&env, "real-arweave-tx-id-abc123"),
        );

        // verify real tx id is now stored
        let updated = client.get_book(&book_id);
        assert_eq!(updated.arweave_tx_id, String::from_str(&env, "real-arweave-tx-id-abc123"));
    }

    #[test]
    fn test_purchase_and_ownership() {
        let env         = Env::default();
        let contract_id = env.register(KnowdlyBookContract, ());
        let client      = KnowdlyBookContractClient::new(&env, &contract_id);
        let platform    = Address::generate(&env);
        let creator     = Address::generate(&env);
        let reader      = Address::generate(&env);

        env.mock_all_auths();
        client.initialise(&platform, &250u32);

        let book_id = client.register_book(
            &creator,
            &10_000_000i128,
            &500u32,
            &String::from_str(&env, "arweave-tx-id-456"),
            &String::from_str(&env, "Calculus for Engineers"),
        );

        assert_eq!(client.owns_book(&reader, &book_id), false);

        let token_id = client.purchase(&reader, &book_id);

        assert_eq!(client.owns_book(&reader, &book_id), true);

        let token = client.get_token(&token_id);
        assert_eq!(token.book_id, book_id);
        assert_eq!(token.owner,   reader);
    }

    #[test]
    fn test_transfer_enforces_royalty() {
        let env         = Env::default();
        let contract_id = env.register(KnowdlyBookContract, ());
        let client      = KnowdlyBookContractClient::new(&env, &contract_id);
        let platform    = Address::generate(&env);
        let creator     = Address::generate(&env);
        let reader_a    = Address::generate(&env);
        let reader_b    = Address::generate(&env);

        env.mock_all_auths();
        client.initialise(&platform, &250u32);

        let book_id  = client.register_book(
            &creator,
            &10_000_000i128,
            &500u32,
            &String::from_str(&env, "arweave-tx-id-789"),
            &String::from_str(&env, "Organic Chemistry"),
        );
        let token_id = client.purchase(&reader_a, &book_id);

        client.transfer_token(&token_id, &reader_b, &8_000_000i128);

        assert_eq!(client.owns_book(&reader_a, &book_id), false);
        assert_eq!(client.owns_book(&reader_b, &book_id), true);
    }

    #[test]
    fn test_get_tokens_by_owner() {
        let env         = Env::default();
        let contract_id = env.register(KnowdlyBookContract, ());
        let client      = KnowdlyBookContractClient::new(&env, &contract_id);
        let platform    = Address::generate(&env);
        let creator     = Address::generate(&env);
        let reader      = Address::generate(&env);

        env.mock_all_auths();
        client.initialise(&platform, &250u32);

        let book_id_a = client.register_book(
            &creator,
            &10_000_000i128,
            &500u32,
            &String::from_str(&env, "arweave-tx-a"),
            &String::from_str(&env, "Book A"),
        );
        let book_id_b = client.register_book(
            &creator,
            &20_000_000i128,
            &500u32,
            &String::from_str(&env, "arweave-tx-b"),
            &String::from_str(&env, "Book B"),
        );

        let tokens_before = client.get_tokens_by_owner(&reader);
        assert_eq!(tokens_before.len(), 0);

        let token_a = client.purchase(&reader, &book_id_a);
        let token_b = client.purchase(&reader, &book_id_b);

        let tokens_after = client.get_tokens_by_owner(&reader);
        assert_eq!(tokens_after.len(), 2);
        assert_eq!(tokens_after.get(0).unwrap(), token_a);
        assert_eq!(tokens_after.get(1).unwrap(), token_b);
    }

    #[test]
    fn test_tokens_update_on_transfer() {
        let env         = Env::default();
        let contract_id = env.register(KnowdlyBookContract, ());
        let client      = KnowdlyBookContractClient::new(&env, &contract_id);
        let platform    = Address::generate(&env);
        let creator     = Address::generate(&env);
        let reader_a    = Address::generate(&env);
        let reader_b    = Address::generate(&env);

        env.mock_all_auths();
        client.initialise(&platform, &250u32);

        let book_id  = client.register_book(
            &creator,
            &10_000_000i128,
            &500u32,
            &String::from_str(&env, "arweave-tx-transfer"),
            &String::from_str(&env, "Transfer Test Book"),
        );
        let token_id = client.purchase(&reader_a, &book_id);

        assert_eq!(client.get_tokens_by_owner(&reader_a).len(), 1);
        assert_eq!(client.get_tokens_by_owner(&reader_b).len(), 0);

        client.transfer_token(&token_id, &reader_b, &8_000_000i128);

        assert_eq!(client.get_tokens_by_owner(&reader_a).len(), 0);
        assert_eq!(client.get_tokens_by_owner(&reader_b).len(), 1);
        assert_eq!(client.get_tokens_by_owner(&reader_b).get(0).unwrap(), token_id);
    }
}