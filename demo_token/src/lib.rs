//! A public fungible test token that anyone can mint, used by the Allowance demo and tests.

use tari_template_lib::prelude::*;

#[template]
mod demo_token {
    use super::*;

    pub struct DemoToken {
        resource: ResourceAddress,
    }

    impl DemoToken {
        pub fn new(symbol: String) -> Component<Self> {
            let resource = ResourceBuilder::public_fungible()
                .with_token_symbol(&symbol)
                .metadata("name", "Allowance demo token (test only)")
                .with_divisibility(2)
                .mintable(AccessRule::AllowAll, OWNER)
                .build();
            Component::new(Self { resource })
                .with_access_rules(AccessRules::allow_all())
                .create()
        }

        /// Mints test tokens to the caller. Testnet only; the token has no value.
        pub fn mint(&mut self, amount: Amount) -> Bucket {
            ResourceManager::get(self.resource).mint_fungible(amount)
        }

        pub fn resource(&self) -> ResourceAddress {
            self.resource
        }
    }
}
