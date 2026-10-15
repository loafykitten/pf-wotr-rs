//! Navigation import: the A* Pathfinding Project graph cache from `.nav` bundles
//! (`graph`), the runtime [`wr_nav::world::World`] built from it (`build`), and `path`, the
//! older approximate baseline. The runtime itself (searches, nearest-node rules, modifiers,
//! queries) is `wr-nav`.

pub mod build;
pub mod graph;
pub mod path;
