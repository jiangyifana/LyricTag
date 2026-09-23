//! 歌词格式处理：解析、时间轴合并、逐字降级、渲染、归一化。

pub mod merge;
pub mod normalize;
pub mod parse;
pub mod render;
pub mod verbatim;

pub use parse::{parse, ParsedLrc};
pub use render::{render_lrc, RenderOptions};
