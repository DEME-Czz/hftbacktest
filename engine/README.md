# Engine

纯 Rust L2 HFT 计算核心。

负责：market replay、L2 depth、queue position、latency、partial fill、fee、position/PnL 与 Strategy API。

不负责：交易所协议、HTTP、WebSocket、Tokio runtime、API Key、AWS、数据库或 CLI。
