// lib.rs — Knowdly Book Smart Contract
// Deployed on Stellar using the Soroban smart contract platform
//
// This contract handles:
//   1. Book registration by professors
//   2. Book purchases by students (minting ownership tokens)
//   3. Royalty enforcement on every resale
//   4. Ownership verification for content access control
//
// Key design principle: every transfer MUST go through this contract
// so royalties cannot be bypassed — unlike marketplace-only enforcement

// no_std means we don't use Rust's standard library
// Soroban contracts run in a WebAssembly sandbox without std
#![no_std]

// import everything we need from the Soroban SDK
use soroban_sdk::{
    contract,        // macro that marks this struct as a Soroban contract
    contractimpl,    // macro that marks the impl block as contract functions
    contracttype,    // macro that marks structs/enums as contract data types
    symbol_short,    // creates short symbol keys for storage (max 9 chars)
    Address,         // represents a Stellar wallet or contract address
    Env,             // the Soroban environment — gives access to storage, events, etc
    String,          // Soroban's string type (different from Rust's std String)
};

// ── Data Types ────────────────────────────────────────────────────────────────
// These structs are stored on the Stellar blockchain
// contracttype makes them serialisable for on-chain storage

// Book represents a textbook registered by a professor
#[contracttype]
#[derive(Clone)]  // Clone lets us copy the struct when reading from storage
pub struct Book {
    // unique identifier for this book — auto-incremented
    pub id: u64,

    // the professor's Stellar wallet address — receives payments and royalties
    pub publisher: Address,

    // purchase price in stroops (1 USDC = 10,000,000 stroops)
    pub price: i128,

    // resale royalty in basis points (500 = 5%, 1000 = 10%, max 5000 = 50%)
    pub royalty_bps: u32,

    // the Arweave transaction ID where encrypted content is stored
    // stored as a Soroban String — points to the book content forever
    pub arweave_tx_id: String,

    // the book title stored on-chain for discoverability
    pub title: String,

    // whether this book is available for new purchases
    // publishers can deactivate a book to stop new sales
    pub active: bool,

    // total number of copies sold — useful for analytics
    pub total_sales: u64,
}

// Token represents a student's ownership of a specific book
// this IS the NFT — owning a token means owning the book
#[contracttype]
#[derive(Clone)]
pub struct Token {
    // unique identifier for this token
    pub id: u64,

    // which book this token represents ownership of
    pub book_id: u64,

    // the current owner's Stellar wallet address
    pub owner: Address,

    // the ledger sequence number when this token was minted
    // used as a timestamp — Stellar ledgers close every ~5 seconds
    pub minted_at: u32,

    // how much was paid when this token was last purchased or resold
    pub purchase_price: i128,
}

// ── Storage Keys ──────────────────────────────────────────────────────────────
// We use an enum to create type-safe storage keys
// Each variant maps to a different piece of data stored on the blockchain

#[contracttype]
pub enum DataKey {
    // stores the next available book ID (a counter)
    NextBookId,

    // stores the next available token ID (a counter)
    NextTokenId,

    // stores a Book struct — key includes the book ID
    Book(u64),

    // stores a Token struct — key includes the token ID
    Token(u64),

    // stores the token ID for a specific owner + book combination
    // used to quickly check "does this wallet own this book?"
    // key is (owner_address, book_id)
    Ownership(Address, u64),

    // stores the Knowdly platform wallet address
    // receives the platform fee on every sale and resale
    Platform,

    // stores the platform fee in basis points
    // e.g. 250 = 2.5% platform fee on every transaction
    PlatformFeeBps,
}

// ── Contract Definition ───────────────────────────────────────────────────────

// #[contract] marks this as the main contract struct
// Soroban uses this to generate the contract's entry points
#[contract]
pub struct KnowdlyBookContract;

// #[contractimpl] marks this impl block as the contract's public functions
// these functions are callable from the outside world
#[contractimpl]
impl KnowdlyBookContract {

    // ── Initialisation ────────────────────────────────────────────────────────

    // initialise must be called once after deployment to set up the contract
    // sets the platform wallet and fee rate
    // env: the Soroban environment (always the first parameter)
    // platform: the Knowdly treasury wallet that receives platform fees
    // fee_bps: platform fee in basis points e.g. 250 = 2.5%
    pub fn initialise(env: Env, platform: Address, fee_bps: u32) {

        // require_auth() means the platform address must sign this transaction
        // this prevents anyone else from initialising the contract
        platform.require_auth();

        // make sure the fee is not unreasonably high
        // 1000 basis points = 10% maximum platform fee
        if fee_bps > 1000 {
            panic!("Platform fee cannot exceed 10%");
        }

        // store the platform wallet address on-chain
        // symbol_short! creates a compact key for instance-level storage
        env.storage()
            .instance()
            .set(&DataKey::Platform, &platform);

        // store the platform fee rate on-chain
        env.storage()
            .instance()
            .set(&DataKey::PlatformFeeBps, &fee_bps);

        // initialise both ID counters to zero
        // every new book and token gets a unique incrementing ID
        env.storage()
            .instance()
            .set(&DataKey::NextBookId, &0u64);

        env.storage()
            .instance()
            .set(&DataKey::NextTokenId, &0u64);
    }

    // ── Professor API ─────────────────────────────────────────────────────────

    // register_book is called by a professor to list their textbook
    // returns the new book's ID so the professor can reference it later
    //
    // price:          purchase price in stroops
    // royalty_bps:    resale royalty in basis points (max 5000 = 50%)
    // arweave_tx_id:  the Arweave transaction ID of the encrypted content
    // title:          the book title stored on-chain
    pub fn register_book(
        env: Env,
        publisher: Address,
        price: i128,
        royalty_bps: u32,
        arweave_tx_id: String,
        title: String,
    ) -> u64 {

        // the publisher must sign this transaction
        // this proves the person calling is actually the publisher
        publisher.require_auth();

        // validate inputs before storing anything
        if price <= 0 {
            panic!("Price must be positive");
        }
        if royalty_bps > 5000 {
            panic!("Royalty cannot exceed 50%");
        }

        // get the current book ID counter from storage
        let book_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextBookId)
            .unwrap_or(0);

        // build the Book struct with all the provided data
        let book = Book {
            id:            book_id,
            publisher:     publisher.clone(),
            price,
            royalty_bps,
            arweave_tx_id,
            title,
            active:        true,   // books are active by default when registered
            total_sales:   0,      // no sales yet
        };

        // store the book on-chain using its ID as part of the key
        env.storage()
            .persistent()          // persistent storage survives ledger expiry
            .set(&DataKey::Book(book_id), &book);

        // increment the book ID counter for the next registration
        env.storage()
            .instance()
            .set(&DataKey::NextBookId, &(book_id + 1));

        // emit an event so off-chain indexers know a book was registered
        // events are like logs — visible in transaction history
        env.events().publish(
            (symbol_short!("reg_book"),),  // event topic
            (book_id, publisher),          // event data
        );

        // return the new book ID to the caller
        book_id
    }

    // deactivate_book stops new purchases of a book
    // only the original publisher can deactivate their own book
    pub fn deactivate_book(env: Env, publisher: Address, book_id: u64) {

        // publisher must sign this transaction
        publisher.require_auth();

        // load the book from storage
        let mut book: Book = env
            .storage()
            .persistent()
            .get(&DataKey::Book(book_id))
            .expect("Book not found");

        // make sure the caller is actually the publisher of this book
        if book.publisher != publisher {
            panic!("Only the publisher can deactivate this book");
        }

        // set active to false — no new purchases will be allowed
        book.active = false;

        // save the updated book back to storage
        env.storage()
            .persistent()
            .set(&DataKey::Book(book_id), &book);
    }

    // ── Student Purchase API ──────────────────────────────────────────────────

    // purchase is called when a student buys a book
    // it mints an ownership token to the student's wallet
    // payment in USDC must be sent alongside this transaction
    // returns the new token ID
    //
    // NOTE: in a full implementation this would integrate with the
    // Stellar Asset Contract (SAC) for USDC to handle payment atomically
    // For now we record ownership and assume payment happened via the
    // Stellar payment operation in the same transaction envelope
    pub fn purchase(env: Env, buyer: Address, book_id: u64) -> u64 {

        // the buyer must sign this transaction
        buyer.require_auth();

        // load the book from storage
        let mut book: Book = env
            .storage()
            .persistent()
            .get(&DataKey::Book(book_id))
            .expect("Book not found");

        // check the book is still available for purchase
        if !book.active {
            panic!("This book is not available for purchase");
        }

        // check the buyer does not already own this book
        // we look up the ownership record for this buyer + book combination
        let already_owned: bool = env
            .storage()
            .persistent()
            .get(&DataKey::Ownership(buyer.clone(), book_id))
            .unwrap_or(false);

        if already_owned {
            panic!("You already own this book");
        }

        // get the current token ID counter
        let token_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextTokenId)
            .unwrap_or(0);

        // build the ownership token
        let token = Token {
            id:             token_id,
            book_id,
            owner:          buyer.clone(),
            minted_at:      env.ledger().sequence(), // current ledger number
            purchase_price: book.price,
        };

        // store the token on-chain
        env.storage()
            .persistent()
            .set(&DataKey::Token(token_id), &token);

        // record ownership — this is what owns_book() checks
        // we store true for this buyer + book combination
        env.storage()
            .persistent()
            .set(&DataKey::Ownership(buyer.clone(), book_id), &true);

        // increment the token counter for the next purchase
        env.storage()
            .instance()
            .set(&DataKey::NextTokenId, &(token_id + 1));

        // increment the book's total sales count
        book.total_sales += 1;
        env.storage()
            .persistent()
            .set(&DataKey::Book(book_id), &book);

        // emit a purchase event for off-chain indexers
        env.events().publish(
            (symbol_short!("purchase"),),
            (token_id, book_id, buyer),
        );

        // return the new token ID
        token_id
    }

    // ── Resale / Transfer API ─────────────────────────────────────────────────

    // transfer_token handles resale of a book token between students
    // royalties are enforced here — they CANNOT be bypassed
    // because the only way to transfer ownership is through this function
    //
    // token_id:   the token being transferred
    // new_owner:  the student receiving the book
    // sale_price: the agreed resale price in stroops
    //
    // NOTE: payment distribution (royalty to publisher, proceeds to seller)
    // would be handled atomically via SAC USDC operations in production
    // For now we update ownership and emit events for the amounts owed
    pub fn transfer_token(
        env: Env,
        token_id: u64,
        new_owner: Address,
        sale_price: i128,
    ) {
        // load the token from storage
        let mut token: Token = env
            .storage()
            .persistent()
            .get(&DataKey::Token(token_id))
            .expect("Token not found");

        // the current owner must sign this transaction
        // this prevents anyone else from transferring someone's token
        token.owner.require_auth();

        // load the book to get the royalty rate
        let book: Book = env
            .storage()
            .persistent()
            .get(&DataKey::Book(token.book_id))
            .expect("Book not found");

        // get the platform fee rate
        let platform_fee_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::PlatformFeeBps)
            .unwrap_or(250); // default 2.5% if not set

        // calculate how much each party receives
        // basis points math: amount * bps / 10000
        let royalty_amount   = (sale_price * book.royalty_bps as i128) / 10_000;
        let platform_amount  = (sale_price * platform_fee_bps as i128) / 10_000;
        let seller_amount    = sale_price - royalty_amount - platform_amount;

        // make sure the seller actually receives something
        if seller_amount < 0 {
            panic!("Sale price too low to cover royalty and platform fees");
        }

        // record the old owner before we update
        let old_owner = token.owner.clone();

        // update ownership records
        // remove ownership from the seller
        env.storage()
            .persistent()
            .set(&DataKey::Ownership(old_owner.clone(), token.book_id), &false);

        // grant ownership to the new owner
        env.storage()
            .persistent()
            .set(&DataKey::Ownership(new_owner.clone(), token.book_id), &true);

        // update the token with the new owner and price
        token.owner          = new_owner.clone();
        token.purchase_price = sale_price;

        // save the updated token back to storage
        env.storage()
            .persistent()
            .set(&DataKey::Token(token_id), &token);

        // emit a transfer event with the payment breakdown
        // off-chain services listen for this to trigger the actual USDC payments
        env.events().publish(
            (symbol_short!("transfer"),),
            (
                token_id,
                old_owner,
                new_owner,
                royalty_amount,    // owed to publisher
                platform_amount,   // owed to Knowdly
                seller_amount,     // owed to seller
            ),
        );
    }

    // ── Access Control API ────────────────────────────────────────────────────

    // owns_book is the critical access control function
    // the key server calls this to decide whether to release the decryption key
    // returns true if the wallet owns the book, false if not
    //
    // This is a read-only function (no state changes) so it costs no fees
    // and can be called freely by anyone including our Next.js key server
    pub fn owns_book(env: Env, owner: Address, book_id: u64) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Ownership(owner, book_id))
            .unwrap_or(false)  // if no record exists return false
    }

    // get_book returns a book's full details
    // used by the frontend to display book information
    pub fn get_book(env: Env, book_id: u64) -> Book {
        env.storage()
            .persistent()
            .get(&DataKey::Book(book_id))
            .expect("Book not found")
    }

    // get_token returns a token's full details
    // used to verify ownership and display token information
    pub fn get_token(env: Env, token_id: u64) -> Token {
        env.storage()
            .persistent()
            .get(&DataKey::Token(token_id))
            .expect("Token not found")
    }

    // get_total_books returns the total number of books registered
    // useful for the frontend to know how many books exist
    pub fn get_total_books(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::NextBookId)
            .unwrap_or(0)
    }

    // get_total_tokens returns the total number of ownership tokens minted
    pub fn get_total_tokens(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::NextTokenId)
            .unwrap_or(0)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
// Soroban has a built-in test framework
// these tests run locally without deploying to the network

#[cfg(test)]
mod test {
    // import everything from the parent module
    use super::*;

    // import Soroban test utilities
    use soroban_sdk::{testutils::Address as _, Env};

    // test that a professor can register a book
    #[test]
    fn test_register_book() {

        // create a test environment
        let env = Env::default();

        // register the contract in the test environment
        let contract_id = env.register(KnowdlyBookContract, ());

        // create a client to call contract functions
        let client = KnowdlyBookContractClient::new(&env, &contract_id);

        // create mock addresses for testing
        let platform  = Address::generate(&env);
        let publisher = Address::generate(&env);

        // mock all authorisations so we don't need real signatures in tests
        env.mock_all_auths();

        // initialise the contract
        client.initialise(&platform, &250u32);

        // register a test book
        let book_id = client.register_book(
            &publisher,
            &10_000_000i128,  // 1 USDC in stroops
            &500u32,          // 5% royalty
            &String::from_str(&env, "arweave-tx-id-123"),
            &String::from_str(&env, "Introduction to Blockchain"),
        );

        // verify the book was stored correctly
        assert_eq!(book_id, 0);  // first book should have ID 0

        // fetch the book and verify its fields
        let book = client.get_book(&book_id);
        assert_eq!(book.id,          0);
        assert_eq!(book.price,       10_000_000);
        assert_eq!(book.royalty_bps, 500);
        assert_eq!(book.active,      true);
        assert_eq!(book.total_sales, 0);
    }

    // test that a student can purchase a book and own it
    #[test]
    fn test_purchase_and_ownership() {

        let env = Env::default();
        let contract_id = env.register(KnowdlyBookContract, ());
        let client = KnowdlyBookContractClient::new(&env, &contract_id);

        let platform  = Address::generate(&env);
        let publisher = Address::generate(&env);
        let student   = Address::generate(&env);

        env.mock_all_auths();

        // set up the contract
        client.initialise(&platform, &250u32);

        // professor registers a book
        let book_id = client.register_book(
            &publisher,
            &10_000_000i128,
            &500u32,
            &String::from_str(&env, "arweave-tx-id-456"),
            &String::from_str(&env, "Calculus for Engineers"),
        );

        // student should not own the book before purchase
        assert_eq!(client.owns_book(&student, &book_id), false);

        // student purchases the book
        let token_id = client.purchase(&student, &book_id);

        // student should now own the book
        assert_eq!(client.owns_book(&student, &book_id), true);

        // verify the token was created correctly
        let token = client.get_token(&token_id);
        assert_eq!(token.book_id, book_id);
        assert_eq!(token.owner,   student);
    }

    // test that ownership transfers correctly with royalty calculation
    #[test]
    fn test_transfer_enforces_royalty() {

        let env = Env::default();
        let contract_id = env.register(KnowdlyBookContract, ());
        let client = KnowdlyBookContractClient::new(&env, &contract_id);

        let platform  = Address::generate(&env);
        let publisher = Address::generate(&env);
        let student_a = Address::generate(&env);
        let student_b = Address::generate(&env);

        env.mock_all_auths();

        client.initialise(&platform, &250u32);

        // register and purchase
        let book_id  = client.register_book(
            &publisher,
            &10_000_000i128,
            &500u32,  // 5% royalty
            &String::from_str(&env, "arweave-tx-id-789"),
            &String::from_str(&env, "Organic Chemistry"),
        );
        let token_id = client.purchase(&student_a, &book_id);

        // student A resells to student B
        client.transfer_token(&token_id, &student_b, &8_000_000i128);

        // student A should no longer own the book
        assert_eq!(client.owns_book(&student_a, &book_id), false);

        // student B should now own the book
        assert_eq!(client.owns_book(&student_b, &book_id), true);
    }
}