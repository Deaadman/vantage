//! Vantage core: turns a `vrfkit export` directory into facts about one
//! player's play. No CLI concerns live here.
#![recursion_limit = "256"]

pub mod agents;
pub mod ai;
pub mod analysis;
pub mod builds;
pub mod catalog;
pub mod clips;
pub mod clusters;
pub mod coach;
pub mod config;
pub mod context;
pub mod deaths;
pub mod export;
pub mod facts;
pub mod fields;
pub mod game;
pub mod habits;
pub mod install;
pub mod items;
pub mod minimap;
pub mod paths;
pub mod plan;
pub mod ranks;
pub mod review;
pub mod scan;
pub mod sections;
pub mod select;
pub mod standing;
pub mod store;
pub mod summary;
pub mod time;
pub mod tracks;
pub mod trends;
pub mod update;
pub mod vrfkit;
pub mod vrfkit_export;

pub use catalog::Catalog;
pub use export::MatchReview;
pub use game::Game;
pub use vrfkit_export::Export;
