use serde::{Deserialize, Deserializer, Serialize};

use crate::rpc::ModelName;
use crate::usage::{CostUsd, TokenCount, TokenMeasure};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTokenPrices {
    pub model: ModelName,
    pub input_token: CostUsd,
    pub output_token: CostUsd,
    pub cache_token: CostUsd,
}

impl ModelTokenPrices {
    pub fn new(
        model: ModelName,
        input_token: CostUsd,
        output_token: CostUsd,
        cache_token: CostUsd,
    ) -> Self {
        Self {
            model,
            input_token,
            output_token,
            cache_token,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PriceTable {
    prices: Vec<ModelTokenPrices>,
}

impl PriceTable {
    pub fn bundled_defaults() -> Self {
        Self {
            prices: vec![
                claude_opus_4_prices("claude-opus-4-20250514"),
                claude_opus_4_prices("claude-opus-4"),
                claude_sonnet_4_prices("claude-sonnet-4-20250514"),
                claude_sonnet_4_prices("claude-sonnet-4"),
            ],
        }
    }

    pub fn from_prices(prices: Vec<ModelTokenPrices>) -> Self {
        let mut table = Self { prices: Vec::new() };
        for price in prices {
            table = table.with_price(price);
        }
        table
    }

    pub fn with_price(mut self, price: ModelTokenPrices) -> Self {
        self.prices.retain(|entry| entry.model != price.model);
        self.prices.push(price);
        self
    }

    pub fn price_for(&self, model: &ModelName) -> Option<&ModelTokenPrices> {
        self.prices.iter().find(|price| price.model == *model)
    }

    pub fn estimate_cost(
        &self,
        model: &ModelName,
        input_tokens: TokenCount,
        output_tokens: TokenCount,
        cache_tokens: TokenCount,
    ) -> Option<CostUsd> {
        self.price_for(model)?
            .estimate_cost(input_tokens, output_tokens, cache_tokens)
    }
}

impl Default for PriceTable {
    fn default() -> Self {
        Self::bundled_defaults()
    }
}

impl<'de> Deserialize<'de> for PriceTable {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct SerializedPriceTable {
            prices: Vec<ModelTokenPrices>,
        }

        let table = SerializedPriceTable::deserialize(deserializer)?;
        Ok(Self::from_prices(table.prices))
    }
}

impl ModelTokenPrices {
    pub fn price_for_measure(&self, measure: TokenMeasure) -> CostUsd {
        match measure {
            TokenMeasure::Input => self.input_token,
            TokenMeasure::Output => self.output_token,
            TokenMeasure::Cache => self.cache_token,
        }
    }

    pub fn estimate_cost(
        &self,
        input_tokens: TokenCount,
        output_tokens: TokenCount,
        cache_tokens: TokenCount,
    ) -> Option<CostUsd> {
        let input_cost = cost_for_tokens(self.input_token, input_tokens)?;
        let output_cost = cost_for_tokens(self.output_token, output_tokens)?;
        let cache_cost = cost_for_tokens(self.cache_token, cache_tokens)?;
        input_cost
            .checked_add(output_cost)
            .and_then(|cost| cost.checked_add(cache_cost))
    }
}

fn claude_opus_4_prices(model: &str) -> ModelTokenPrices {
    ModelTokenPrices::new(
        ModelName::new(model),
        CostUsd::from_nanos(15_000).expect("default price must fit storage"),
        CostUsd::from_nanos(75_000).expect("default price must fit storage"),
        CostUsd::from_nanos(1_500).expect("default price must fit storage"),
    )
}

fn claude_sonnet_4_prices(model: &str) -> ModelTokenPrices {
    ModelTokenPrices::new(
        ModelName::new(model),
        CostUsd::from_nanos(3_000).expect("default price must fit storage"),
        CostUsd::from_nanos(15_000).expect("default price must fit storage"),
        CostUsd::from_nanos(300).expect("default price must fit storage"),
    )
}

fn cost_for_tokens(price: CostUsd, tokens: TokenCount) -> Option<CostUsd> {
    price.checked_mul(tokens.value())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_price_table_computes_representative_claude_model_costs() {
        let table = PriceTable::bundled_defaults();

        assert_eq!(
            table.estimate_cost(
                &ModelName::new("claude-opus-4-20250514"),
                TokenCount::new(1_000),
                TokenCount::new(200),
                TokenCount::new(50),
            ),
            CostUsd::from_nanos(30_075_000)
        );
        assert_eq!(
            table.estimate_cost(
                &ModelName::new("claude-sonnet-4-20250514"),
                TokenCount::new(1_000),
                TokenCount::new(200),
                TokenCount::new(50),
            ),
            CostUsd::from_nanos(6_015_000)
        );
    }

    #[test]
    fn price_table_accepts_user_supplied_model_prices() {
        let table = PriceTable::from_prices(vec![ModelTokenPrices::new(
            ModelName::new("local-test-model"),
            CostUsd::from_nanos(10).unwrap(),
            CostUsd::from_nanos(20).unwrap(),
            CostUsd::from_nanos(5).unwrap(),
        )]);

        assert_eq!(
            table.estimate_cost(
                &ModelName::new("local-test-model"),
                TokenCount::new(100),
                TokenCount::new(10),
                TokenCount::new(4),
            ),
            CostUsd::from_nanos(1_220)
        );
        assert_eq!(
            table.estimate_cost(
                &ModelName::new("model-without-user-price"),
                TokenCount::new(100),
                TokenCount::new(10),
                TokenCount::new(4),
            ),
            None
        );
    }

    #[test]
    fn price_table_uses_the_last_duplicate_model_price() {
        let table = PriceTable::from_prices(vec![
            ModelTokenPrices::new(
                ModelName::new("local-test-model"),
                CostUsd::from_nanos(10).unwrap(),
                CostUsd::from_nanos(20).unwrap(),
                CostUsd::from_nanos(5).unwrap(),
            ),
            ModelTokenPrices::new(
                ModelName::new("local-test-model"),
                CostUsd::from_nanos(100).unwrap(),
                CostUsd::from_nanos(200).unwrap(),
                CostUsd::from_nanos(50).unwrap(),
            ),
        ]);

        assert_eq!(
            table.estimate_cost(
                &ModelName::new("local-test-model"),
                TokenCount::new(1),
                TokenCount::new(1),
                TokenCount::new(1),
            ),
            CostUsd::from_nanos(350)
        );
    }

    #[test]
    fn deserialized_price_table_canonicalizes_duplicate_model_prices()
    -> Result<(), Box<dyn std::error::Error>> {
        let table: PriceTable = serde_json::from_value(serde_json::json!({
            "prices": [
                {
                    "model": "local-test-model",
                    "input_token": { "nanos": 10u64 },
                    "output_token": { "nanos": 20u64 },
                    "cache_token": { "nanos": 5u64 }
                },
                {
                    "model": "local-test-model",
                    "input_token": { "nanos": 100u64 },
                    "output_token": { "nanos": 200u64 },
                    "cache_token": { "nanos": 50u64 }
                }
            ]
        }))?;

        assert_eq!(
            table.estimate_cost(
                &ModelName::new("local-test-model"),
                TokenCount::new(1),
                TokenCount::new(1),
                TokenCount::new(1),
            ),
            CostUsd::from_nanos(350)
        );

        Ok(())
    }
}
