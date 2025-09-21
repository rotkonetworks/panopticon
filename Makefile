# panopticon deployment makefile
CHAIN_RPC ?= https://eth-passet-hub-paseo.dotters.network
ALICE = 0xf24FF3a9CF04c71Dbc94D0b566f7A27B94566cac
BOB = 0x3Cd0A705a2DC65e5b1E1205896BaA2be8A07c6e0

# colors
RED = \033[0;31m
GREEN = \033[0;32m
YELLOW = \033[1;33m
NC = \033[0m

.PHONY: all build clean deploy test balance withdraw

all: build

build:
	@echo "$(YELLOW)Building Panopticon contract...$(NC)"
	@RUSTC_BOOTSTRAP=1 cargo build --release
	@polkatool link --strip --output panopticon.polkavm target/riscv64emac-unknown-none-polkavm/release/contract
	@echo "$(GREEN)Contract built: panopticon.polkavm$(NC)"

deploy: build
	@echo "$(YELLOW)Deploying Panopticon to Paseo...$(NC)"
	@if [ ! -f .env.example ]; then \
		echo "$(RED)Error: .env.example not found$(NC)"; \
		exit 1; \
	fi
	@. ./.env.example && \
	cast wallet import panopticon-deployer --private-key $$PRIVATE_KEY 2>/dev/null || true && \
	CONTRACT=$$(cast send --account panopticon-deployer --rpc-url $(CHAIN_RPC) \
		--create "$$(xxd -p -c 99999 panopticon.polkavm)" \
		--json | jq -r .contractAddress) && \
	echo "CONTRACT_ADDRESS=$$CONTRACT" > .env.deployed && \
	echo "$(GREEN)Deployed at: $$CONTRACT$(NC)"

test: 
	@if [ ! -f .env.deployed ]; then \
		echo "$(RED)Error: Deploy first with 'make deploy'$(NC)"; \
		exit 1; \
	fi
	@. ./.env.deployed && \
	echo "$(YELLOW)Testing route from Alice to Bob...$(NC)" && \
	echo "  Sending: 1.1 KSM (1 KSM + 0.1 KSM fee)" && \
	TX=$$(cast send --account panopticon-deployer --rpc-url $(CHAIN_RPC) \
		$$CONTRACT_ADDRESS \
		"route(address)" $$(echo $(BOB)) \
		--value 1.1ether \
		--json | jq -r .transactionHash) && \
	echo "$(GREEN)Transaction: $$TX$(NC)"

balance:
	@echo "$(YELLOW)Checking balances...$(NC)"
	@echo "Alice: $$(cast balance --rpc-url $(CHAIN_RPC) $(ALICE) --ether) KSM"
	@echo "Bob:   $$(cast balance --rpc-url $(CHAIN_RPC) $(BOB) --ether) KSM"
	@if [ -f .env.deployed ]; then \
		. ./.env.deployed && \
		echo "Router: $$(cast balance --rpc-url $(CHAIN_RPC) $$CONTRACT_ADDRESS --ether) KSM"; \
	fi

withdraw:
	@if [ ! -f .env.deployed ]; then \
		echo "$(RED)Error: Deploy first with 'make deploy'$(NC)"; \
		exit 1; \
	fi
	@. ./.env.deployed && \
	echo "$(YELLOW)Withdrawing fees...$(NC)" && \
	cast send --account panopticon-deployer --rpc-url $(CHAIN_RPC) \
		$$CONTRACT_ADDRESS \
		"withdraw()"

logs:
	@if [ ! -f .env.deployed ]; then \
		echo "$(RED)Error: Deploy first with 'make deploy'$(NC)"; \
		exit 1; \
	fi
	@. ./.env.deployed && \
	cast logs --rpc-url $(CHAIN_RPC) \
		--address $$CONTRACT_ADDRESS \
		--from-block latest

clean:
	@cargo clean
	@rm -f panopticon.polkavm .env.deployed
	@echo "$(GREEN)Cleaned build artifacts$(NC)"

info:
	@echo "$(YELLOW)Panopticon - 12-hop compliance router$(NC)"
	@echo "Fee: 0.1 KSM per routing"
	@echo "Hops: 12 self-destructing cells"
	@if [ -f .env.deployed ]; then \
		. ./.env.deployed && \
		echo "Deployed: $$CONTRACT_ADDRESS"; \
	else \
		echo "Status: Not deployed"; \
	fi

faucet:
	@echo "$(YELLOW)Get test KSM from faucet:$(NC)"
	@echo "https://faucet.polkadot.io/"
	@echo "Your address: $(ALICE)"
