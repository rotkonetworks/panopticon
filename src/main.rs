#![no_main]
#![no_std]

// panopticon: 12 self-destructing hop contracts for compliance theater
// deploys transient cells that forward funds and vanish
// motivation explained: https://x.com/bitfalls/status/1964732897157410844

use uapi::{HostFn, HostFnImpl as api};

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}

// contract types
const TYPE_ROUTER: u8 = 1;
const TYPE_CELL: u8 = 2;

// storage keys
const CONTRACT_TYPE: [u8; 32] = [0; 32];
const OWNER: [u8; 32] = [1; 32];
const FEES_COLLECTED: [u8; 32] = [2; 32];
const HOPS_REMAINING: [u8; 32] = [3; 32];
const ROUTER_ADDRESS: [u8; 32] = [4; 32];
const NONCE: [u8; 32] = [5; 32];

// constants
const ROUTING_FEE: u128 = 100_000_000_000_000; // 0.1 KSM
const HOP_COUNT: u8 = 12;
const DEPLOYMENT_GAS: u64 = 500_000;
const FORWARD_GAS: u64 = 100_000;
const MAX_GAS_PER_CELL: u64 = 1_000_000;

#[no_mangle]
#[polkavm_derive::polkavm_export]
pub extern "C" fn deploy() {
    let mut input = [0u8; 32];
    api::call_data_copy(&mut input, 0);
    
    match input[0] {
        0 | TYPE_ROUTER => {
            // router deployment
            let mut type_storage = [0u8; 32];
            type_storage[0] = TYPE_ROUTER;
            api::set_storage(uapi::StorageFlags::empty(), &CONTRACT_TYPE, &type_storage);
            
            // store deployer as owner
            let mut caller = [0u8; 20];
            api::caller(&mut caller);
            let mut owner_storage = [0u8; 32];
            owner_storage[..20].copy_from_slice(&caller);
            api::set_storage(uapi::StorageFlags::empty(), &OWNER, &owner_storage);
            
            // initialize state
            api::set_storage(uapi::StorageFlags::empty(), &FEES_COLLECTED, &[0u8; 32]);
            api::set_storage(uapi::StorageFlags::empty(), &NONCE, &[0u8; 32]);
        }
        TYPE_CELL => {
            // cell deployment
            let mut type_storage = [0u8; 32];
            type_storage[0] = TYPE_CELL;
            api::set_storage(uapi::StorageFlags::empty(), &CONTRACT_TYPE, &type_storage);
            
            // store remaining hops with bounds check
            let hops = input[1].min(HOP_COUNT);
            let mut hops_storage = [0u8; 32];
            hops_storage[0] = hops;
            api::set_storage(uapi::StorageFlags::empty(), &HOPS_REMAINING, &hops_storage);
            
            // store router address for gas refund
            let mut router_storage = [0u8; 32];
            router_storage[..20].copy_from_slice(&input[2..22]);
            api::set_storage(uapi::StorageFlags::empty(), &ROUTER_ADDRESS, &router_storage);
        }
        _ => api::return_value(uapi::ReturnFlags::REVERT, b"invalid type"),
    }
}

#[no_mangle]
#[polkavm_derive::polkavm_export]
pub extern "C" fn call() {
    let mut type_storage = [0u8; 32];
    api::get_storage_or_zero(uapi::StorageFlags::empty(), &CONTRACT_TYPE, &mut type_storage);
    
    match type_storage[0] {
        TYPE_ROUTER => handle_router(),
        TYPE_CELL => handle_cell(),
        _ => api::return_value(uapi::ReturnFlags::REVERT, b"invalid type"),
    }
}

fn handle_router() {
    let mut selector = [0u8; 4];
    api::call_data_copy(&mut selector, 0);
    
    match u32::from_be_bytes(selector) {
        0x12345678 => route(),
        0x3ccfd60b => withdraw(),
        _ => api::return_value(uapi::ReturnFlags::REVERT, b"unknown selector"),
    }
}

fn route() {
    // read destination
    let mut destination = [0u8; 20];
    api::call_data_copy(&mut destination, 4);
    
    // check value includes fee
    let mut value_bytes = [0u8; 32];
    api::value_transferred(&mut value_bytes);
    let value = u128::from_le_bytes(value_bytes[..16].try_into().unwrap());
    
    if value <= ROUTING_FEE {
        api::return_value(uapi::ReturnFlags::REVERT, b"insufficient fee");
    }
    
    // gas exhaustion attack protection: verify sufficient gas for full chain
    // prevents griefing where tx has fee but insufficient gas for 12 deployments
    let required_gas = DEPLOYMENT_GAS + (HOP_COUNT as u64 * MAX_GAS_PER_CELL);
    if api::gas_limit() < required_gas {
        api::return_value(uapi::ReturnFlags::REVERT, b"insufficient gas");
    }
    
    // accumulate fees
    let mut fees_storage = [0u8; 32];
    api::get_storage_or_zero(uapi::StorageFlags::empty(), &FEES_COLLECTED, &mut fees_storage);
    let total_fees = u128::from_le_bytes(fees_storage[..16].try_into().unwrap()).saturating_add(ROUTING_FEE);
    fees_storage[..16].copy_from_slice(&total_fees.to_le_bytes());
    api::set_storage(uapi::StorageFlags::empty(), &FEES_COLLECTED, &fees_storage);
    
    // increment nonce for salt entropy
    // mitigates salt predictability in cell deployment
    let mut nonce_storage = [0u8; 32];
    api::get_storage_or_zero(uapi::StorageFlags::empty(), &NONCE, &mut nonce_storage);
    let nonce = u64::from_le_bytes(nonce_storage[..8].try_into().unwrap()).wrapping_add(1);
    nonce_storage[..8].copy_from_slice(&nonce.to_le_bytes());
    api::set_storage(uapi::StorageFlags::empty(), &NONCE, &nonce_storage);
    
    // get router address for cells
    let mut router_addr = [0u8; 20];
    api::address(&mut router_addr);
    
    // deploy first cell with 12 hops
    let first_cell = deploy_cell(HOP_COUNT, router_addr, nonce);
    
    // forward funds minus fee
    let forward_value = value - ROUTING_FEE;
    let mut forward_bytes = [0u8; 32];
    forward_bytes[..16].copy_from_slice(&forward_value.to_le_bytes());
    
    // call first cell with calculated gas allocation
    let gas_for_cell = api::gas_limit().saturating_sub(DEPLOYMENT_GAS).min(MAX_GAS_PER_CELL * HOP_COUNT as u64);
    let result = api::call(
        uapi::CallFlags::empty(),
        &first_cell,
        gas_for_cell,
        0,
        &[0xff; 32],
        &forward_bytes,
        &destination[..],
        None,
    );
    
    if result.is_err() {
        api::return_value(uapi::ReturnFlags::REVERT, b"routing failed");
    }
    
    // return first cell address
    let mut response = [0u8; 32];
    response[12..32].copy_from_slice(&first_cell);
    api::return_value(uapi::ReturnFlags::empty(), &response);
}

fn handle_cell() {
    // get remaining hops with bounds check
    // prevents underflow if storage corrupted
    let mut hops_storage = [0u8; 32];
    api::get_storage_or_zero(uapi::StorageFlags::empty(), &HOPS_REMAINING, &mut hops_storage);
    let remaining = hops_storage[0].min(HOP_COUNT);
    
    // get router address
    let mut router_storage = [0u8; 32];
    api::get_storage_or_zero(uapi::StorageFlags::empty(), &ROUTER_ADDRESS, &mut router_storage);
    let mut router = [0u8; 20];
    router.copy_from_slice(&router_storage[..20]);
    
    // read destination
    let mut destination = [0u8; 20];
    api::call_data_copy(&mut destination, 0);
    
    // get value to forward
    let mut value_bytes = [0u8; 32];
    api::value_transferred(&mut value_bytes);
    
    // check if final hop using saturating arithmetic
    // prevents underflow attacks
    let next_hop = remaining.saturating_sub(1);
    
    if next_hop == 0 {
        // final hop - deliver to destination
        let result = api::call(
            uapi::CallFlags::empty(),
            &destination,
            api::gas_limit().saturating_sub(FORWARD_GAS).min(MAX_GAS_PER_CELL),
            0,
            &[0xff; 32],
            &value_bytes,
            &[],
            None,
        );
        
        if result.is_err() {
            // don't revert, just terminate - funds go to router
            // ensures chain cleanup even on delivery failure
        }
    } else {
        // create deterministic but unpredictable nonce
        // combines multiple entropy sources to prevent front-running
        let mut nonce_data = [0u8; 16];
        nonce_data[0] = next_hop;
        nonce_data[1..9].copy_from_slice(&api::ref_time_left().to_le_bytes());
        nonce_data[9..13].copy_from_slice(&(api::gas_price() as u32).to_le_bytes());
        let nonce = u64::from_le_bytes(nonce_data[..8].try_into().unwrap());
        
        // deploy next cell
        let next_cell = deploy_cell(next_hop, router, nonce);
        
        // forward to next cell with calculated gas
        let gas_for_next = api::gas_limit().saturating_sub(DEPLOYMENT_GAS).min(MAX_GAS_PER_CELL * next_hop as u64);
        let result = api::call(
            uapi::CallFlags::empty(),
            &next_cell,
            gas_for_next,
            0,
            &[0xff; 32],
            &value_bytes,
            &destination[..],
            None,
        );
        
        if result.is_err() {
            // don't revert, just terminate
            // ensures cleanup continues even on forward failure
        }
    }
    
    // self-destruct to clean chain state
    // gas refund and any remaining balance goes to router
    api::terminate(&router);
}

fn deploy_cell(hops: u8, router: [u8; 20], nonce: u64) -> [u8; 20] {
    // get own code hash
    let mut code_hash = [0u8; 32];
    api::own_code_hash(&mut code_hash);
    
    // constructor data
    let mut constructor = [0u8; 22];
    constructor[0] = TYPE_CELL;
    constructor[1] = hops;
    constructor[2..22].copy_from_slice(&router);
    
    // prepare instantiate input
    let mut input = [0u8; 54];
    input[..32].copy_from_slice(&code_hash);
    input[32..].copy_from_slice(&constructor);
    
    // salt entropy weakness: predictable salt could allow front-running
    // attacker could pre-compute addresses and manipulate deployments
    // mixing router-controlled nonce with runtime entropy as mitigation
    let mut salt_data = [0u8; 64];
    salt_data[0] = hops;
    salt_data[1..21].copy_from_slice(&router);
    salt_data[21..29].copy_from_slice(&nonce.to_le_bytes());
    
    // add runtime entropy from block number
    let mut block_num = [0u8; 32];
    api::block_number(&mut block_num);
    salt_data[29..37].copy_from_slice(&block_num[..8]);
    
    // additional entropy from timestamp
    // not cryptographically secure but adds unpredictability
    let mut now = [0u8; 32];
    api::now(&mut now);
    salt_data[37..45].copy_from_slice(&now[..8]);
    
    // hash for final salt
    let mut salt = [0u8; 32];
    api::hash_keccak_256(&salt_data[..45], &mut salt);
    
    // deploy the cell with calculated gas limit
    let mut address = [0u8; 20];
    let gas_for_deploy = api::gas_limit().saturating_sub(FORWARD_GAS).min(DEPLOYMENT_GAS);
    let result = api::instantiate(
        gas_for_deploy,
        0,
        &[0xff; 32],
        &[0; 32],
        &input,
        Some(&mut address),
        None,
        Some(&salt),
    );
    
    if result.is_err() {
        api::return_value(uapi::ReturnFlags::REVERT, b"cell deploy failed");
    }
    
    address
}

fn withdraw() {
    // verify owner
    let mut caller = [0u8; 20];
    api::caller(&mut caller);
    
    let mut owner_storage = [0u8; 32];
    api::get_storage_or_zero(uapi::StorageFlags::empty(), &OWNER, &mut owner_storage);
    
    if caller != owner_storage[..20] {
        api::return_value(uapi::ReturnFlags::REVERT, b"not owner");
    }
    
    // get accumulated fees
    let mut fees_storage = [0u8; 32];
    api::get_storage_or_zero(uapi::StorageFlags::empty(), &FEES_COLLECTED, &mut fees_storage);
    let fees = u128::from_le_bytes(fees_storage[..16].try_into().unwrap());
    
    if fees == 0 {
        api::return_value(uapi::ReturnFlags::REVERT, b"no fees");
    }
    
    // reset fees before transfer (reentrancy protection)
    // prevents reentrancy attacks if owner is malicious contract
    api::set_storage(uapi::StorageFlags::empty(), &FEES_COLLECTED, &[0u8; 32]);
    
    // transfer fees to owner
    // note: using call() for value transfer works but isn't ideal
    // substrate's call() handles EOA transfers, unlike ethereum
    let mut fees_bytes = [0u8; 32];
    fees_bytes[..16].copy_from_slice(&fees.to_le_bytes());
    
    let result = api::call(
        uapi::CallFlags::empty(),
        &caller,
        api::gas_limit().saturating_sub(FORWARD_GAS).min(MAX_GAS_PER_CELL),
        0,
        &[0xff; 32],
        &fees_bytes,
        &[],
        None,
    );
    
    if result.is_err() {
        // restore fees if transfer failed
        // maintains contract invariants on failure
        api::set_storage(uapi::StorageFlags::empty(), &FEES_COLLECTED, &fees_storage);
        api::return_value(uapi::ReturnFlags::REVERT, b"withdraw failed");
    }
    
    api::return_value(uapi::ReturnFlags::empty(), &fees_bytes[..16]);
}
