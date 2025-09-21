#![no_main]
#![no_std]

// panopticon: 12-hop compliance router
// each hop deploys a transient cell that forwards and self-destructs

use uapi::{HostFn, HostFnImpl as api};

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}

// contract types determined by constructor
const TYPE_ROUTER: u8 = 1;
const TYPE_CELL: u8 = 2;

// storage keys  
const CONTRACT_TYPE: [u8; 32] = [0; 32];
const OWNER: [u8; 32] = [1; 32];
const FEES_COLLECTED: [u8; 32] = [2; 32];
const REMAINING_HOPS: [u8; 32] = [3; 32];
const ROUTER_ADDRESS: [u8; 32] = [4; 32];

const ROUTING_FEE: u128 = 100_000_000_000_000; // 0.1 KSM

#[no_mangle]
#[polkavm_derive::polkavm_export]
pub extern "C" fn deploy() {
    // read constructor data to determine contract type
    let mut input = [0u8; 32];
    api::call_data_copy(&mut input, 0);
    
    if input[0] == 0 || input[0] == TYPE_ROUTER {
        // router deployment
        let mut type_storage = [0u8; 32];
        type_storage[0] = TYPE_ROUTER;
        api::set_storage(uapi::StorageFlags::empty(), &CONTRACT_TYPE, &type_storage);
        
        let mut caller = [0u8; 20];
        api::caller(&mut caller);
        let mut owner_storage = [0u8; 32];
        owner_storage[..20].copy_from_slice(&caller);
        api::set_storage(uapi::StorageFlags::empty(), &OWNER, &owner_storage);
        
        api::set_storage(uapi::StorageFlags::empty(), &FEES_COLLECTED, &[0u8; 32]);
    } else if input[0] == TYPE_CELL {
        // cell deployment - store hop count and router address
        let mut type_storage = [0u8; 32];
        type_storage[0] = TYPE_CELL;
        api::set_storage(uapi::StorageFlags::empty(), &CONTRACT_TYPE, &type_storage);
        
        let mut hop_storage = [0u8; 32];
        hop_storage[0] = input[1];
        api::set_storage(uapi::StorageFlags::empty(), &REMAINING_HOPS, &hop_storage);
        
        let mut router_storage = [0u8; 32];
        router_storage[..20].copy_from_slice(&input[2..22]);
        api::set_storage(uapi::StorageFlags::empty(), &ROUTER_ADDRESS, &router_storage);
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
    // read destination address from calldata[4:24]
    let mut destination = [0u8; 20];
    api::call_data_copy(&mut destination, 4);
    
    // check value includes fee
    let mut value_bytes = [0u8; 32];
    api::value_transferred(&mut value_bytes);
    let value = u128::from_le_bytes(value_bytes[..16].try_into().unwrap());
    
    if value <= ROUTING_FEE {
        api::return_value(uapi::ReturnFlags::REVERT, b"insufficient fee");
    }
    
    // accumulate fees
    let mut fees_storage = [0u8; 32];
    api::get_storage_or_zero(uapi::StorageFlags::empty(), &FEES_COLLECTED, &mut fees_storage);
    let mut total_fees = u128::from_le_bytes(fees_storage[..16].try_into().unwrap());
    total_fees = total_fees.saturating_add(ROUTING_FEE);
    fees_storage[..16].copy_from_slice(&total_fees.to_le_bytes());
    api::set_storage(uapi::StorageFlags::empty(), &FEES_COLLECTED, &fees_storage);
    
    // get router address for cells
    let mut router_addr = [0u8; 20];
    api::address(&mut router_addr);
    
    // deploy first cell
    let first_cell = deploy_cell(12, router_addr);
    
    // forward funds to first cell
    let forward_value = value - ROUTING_FEE;
    let mut forward_bytes = [0u8; 32];
    forward_bytes[..16].copy_from_slice(&forward_value.to_le_bytes());
    
    let result = api::call(
        uapi::CallFlags::empty(),
        &first_cell,
        api::gas_limit() * 2 / 3,
        0,
        &[0xff; 32],
        &forward_bytes,
        &destination[..],
        None,
    );
    
    if result.is_err() {
        api::return_value(uapi::ReturnFlags::REVERT, b"hop failed");
    }
    
    // return first cell address
    let mut response = [0u8; 32];
    response[12..32].copy_from_slice(&first_cell);
    api::return_value(uapi::ReturnFlags::empty(), &response);
}

fn handle_cell() {
    // get remaining hops
    let mut hop_storage = [0u8; 32];
    api::get_storage_or_zero(uapi::StorageFlags::empty(), &REMAINING_HOPS, &mut hop_storage);
    let remaining = hop_storage[0];
    
    // get router address for gas refund
    let mut router_storage = [0u8; 32];
    api::get_storage_or_zero(uapi::StorageFlags::empty(), &ROUTER_ADDRESS, &mut router_storage);
    let mut router = [0u8; 20];
    router.copy_from_slice(&router_storage[..20]);
    
    // read destination from calldata
    let mut destination = [0u8; 20];
    api::call_data_copy(&mut destination, 0);
    
    // get value to forward
    let mut value_bytes = [0u8; 32];
    api::value_transferred(&mut value_bytes);
    
    if remaining <= 1 {
        // final hop - deliver to destination
        let result = api::call(
            uapi::CallFlags::empty(),
            &destination,
            api::gas_limit() / 2,
            0,
            &[0xff; 32],
            &value_bytes,
            &[],
            None,
        );
        
        if result.is_err() {
            api::return_value(uapi::ReturnFlags::REVERT, b"delivery failed");
        }
    } else {
        // deploy next cell
        let next_cell = deploy_cell(remaining - 1, router);
        
        // forward to next cell
        let result = api::call(
            uapi::CallFlags::empty(),
            &next_cell,
            api::gas_limit() * 2 / 3,
            0,
            &[0xff; 32],
            &value_bytes,
            &destination[..],
            None,
        );
        
        if result.is_err() {
            api::return_value(uapi::ReturnFlags::REVERT, b"forward failed");
        }
    }
    
    // self-destruct to clean chain state
    api::terminate(&router);
}

fn deploy_cell(hops: u8, router: [u8; 20]) -> [u8; 20] {
    // get code hash for self-deployment
    let mut code_hash = [0u8; 32];
    api::own_code_hash(&mut code_hash);
    
    // constructor: type + hops + router
    let mut constructor = [0u8; 22];
    constructor[0] = TYPE_CELL;
    constructor[1] = hops;
    constructor[2..22].copy_from_slice(&router);
    
    // prepare input: code_hash + constructor
    let mut input = [0u8; 54];
    input[..32].copy_from_slice(&code_hash);
    input[32..].copy_from_slice(&constructor);
    
    // unique salt
    let mut salt_data = [0u8; 42];
    salt_data[0] = hops;
    salt_data[1..21].copy_from_slice(&router);
    let mut block_num = [0u8; 32];
    api::block_number(&mut block_num);
    salt_data[21..29].copy_from_slice(&block_num[..8]);
    salt_data[29..37].copy_from_slice(&api::ref_time_left().to_le_bytes());
    salt_data[37..41].copy_from_slice(&api::gas_price().to_le_bytes()[..4]);
    salt_data[41] = (api::gas_limit() % 256) as u8;
    
    let mut salt = [0u8; 32];
    api::hash_keccak_256(&salt_data, &mut salt);
    
    // deploy the cell
    let mut address = [0u8; 20];
    let result = api::instantiate(
        api::gas_limit() / 3,
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
    
    // get fees
    let mut fees_storage = [0u8; 32];
    api::get_storage_or_zero(uapi::StorageFlags::empty(), &FEES_COLLECTED, &mut fees_storage);
    let fees = u128::from_le_bytes(fees_storage[..16].try_into().unwrap());
    
    if fees == 0 {
        api::return_value(uapi::ReturnFlags::REVERT, b"no fees");
    }
    
    // transfer to owner
    let mut fees_bytes = [0u8; 32];
    fees_bytes[..16].copy_from_slice(&fees.to_le_bytes());
    
    let result = api::call(
        uapi::CallFlags::empty(),
        &caller,
        api::gas_limit() / 2,
        0,
        &[0xff; 32],
        &fees_bytes,
        &[],
        None,
    );
    
    if result.is_err() {
        api::return_value(uapi::ReturnFlags::REVERT, b"withdraw failed");
    }
    
    // reset fees
    api::set_storage(uapi::StorageFlags::empty(), &FEES_COLLECTED, &[0u8; 32]);
    
    api::return_value(uapi::ReturnFlags::empty(), &fees_bytes[..16]);
}
