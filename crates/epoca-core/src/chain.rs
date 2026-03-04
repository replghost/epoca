use epoca_chain::ChainClient;

pub struct ChainGlobal {
    pub client: ChainClient,
}

impl gpui::Global for ChainGlobal {}
