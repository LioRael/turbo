#![feature(arbitrary_self_types)]

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use indexmap::IndexSet;
use rustc_hash::FxHashMap;
use turbo_tasks::{TurboTasks, Vc};
use turbo_tasks_fs::{DiskFileSystem, FileContent, FileSystem, FileSystemPath};
use turbo_tasks_memory::MemoryBackend;
use turbopack_core::{
    asset::{Asset, AssetContent},
    ident::AssetIdent,
    module::Module,
};
use turbopack_ecmascript::scope_hoisting::group::{split_scopes, DepGraph, EdgeData};

fn register() {
    turbo_tasks::register();
    turbo_tasks_fs::register();
    turbopack_ecmascript::register();

    include!(concat!(env!("OUT_DIR"), "/register_test_scope_hoisting.rs"));
}

#[tokio::test]
async fn test_1() -> Result<()> {
    let result = split(to_num_deps(vec![
        ("example", vec![("a", false), ("b", false), ("lazy", true)]),
        ("lazy", vec![("c", false), ("d", false)]),
        ("a", vec![("shared", false)]),
        ("c", vec![("shared", false), ("cjs", false)]),
        ("shared", vec![("shared2", false)]),
    ]))
    .await?;

    assert_eq!(result, vec![vec![6, 8], vec![3, 4, 7, 5], vec![0, 1, 2]]);

    Ok(())
}

#[tokio::test]
async fn test_2() -> Result<()> {
    // a => b
    // a => c
    // b => d
    // c => d
    let result = split(to_num_deps(vec![
        ("example", vec![("a", false), ("b", false), ("lazy", true)]),
        ("lazy", vec![("shared", false)]),
        ("a", vec![("shared", false), ("b", false), ("c", false)]),
        ("b", vec![("shared", false), ("d", false)]),
        ("c", vec![("shared", false), ("d", false)]),
        ("d", vec![("shared", false)]),
        ("shared", vec![("shared2", false)]),
    ]))
    .await?;

    assert_eq!(result, vec![vec![6, 8], vec![3, 4, 7, 5], vec![0, 1, 2]]);

    Ok(())
}

fn to_num_deps(deps: Vec<(&str, Vec<(&str, bool)>)>) -> Deps {
    let mut map = IndexSet::new();

    for (from, to) in deps.iter() {
        if map.insert(*from) {
            eprintln!("Inserted {from} as {}", map.get_full(from).unwrap().0);
        }

        for (to, is_lazy) in to {
            if map.insert(to) {
                eprintln!("Inserted {to} as {}", map.get_full(to).unwrap().0);
            }
        }
    }

    deps.into_iter()
        .map(|(from, to)| {
            (
                map.get_full(from).unwrap().0,
                to.into_iter()
                    .map(|(to, is_lazy)| (map.get_full(to).unwrap().0, is_lazy))
                    .collect(),
            )
        })
        .collect()
}

type Deps = Vec<(usize, Vec<(usize, bool)>)>;

async fn split(deps: Deps) -> Result<Vec<Vec<usize>>> {
    register();

    let tt = TurboTasks::new(MemoryBackend::default());
    tt.run_once(async move {
        let fs = DiskFileSystem::new("test".to_owned(), "test".to_owned(), Default::default());

        let graph = test_dep_graph(fs, deps);

        let group = split_scopes(to_module(fs, 0), graph);

        let group = group.await?;

        let mut data = vec![];

        for scope in group.scopes.await?.iter() {
            let mut scope_data = vec![];

            for &module in scope.await?.modules.await?.iter() {
                let module = from_module(module).await?;
                scope_data.push(module);
            }

            data.push(scope_data);
        }

        Ok(data)
    })
    .await
}

fn test_dep_graph(fs: Vc<DiskFileSystem>, deps: Deps) -> Vc<Box<dyn DepGraph>> {
    let mut dependants = HashMap::new();
    let mut lazy = HashSet::new();

    for (from, to) in &deps {
        for &(to, is_lazy) in to {
            dependants.entry(to).or_insert_with(Vec::new).push(*from);

            if is_lazy {
                lazy.insert((*from, to));
            }
        }
    }

    Vc::upcast(
        TestDepGraph {
            fs,
            deps: deps.into_iter().collect(),
            dependants,
            lazy,
        }
        .cell(),
    )
}

#[turbo_tasks::value]
pub struct TestDepGraph {
    fs: Vc<DiskFileSystem>,
    deps: HashMap<usize, Vec<(usize, bool)>>,
    dependants: HashMap<usize, Vec<usize>>,
    lazy: HashSet<(usize, usize)>,
}

fn to_module(fs: Vc<DiskFileSystem>, id: usize) -> Vc<Box<dyn Module>> {
    let vc = TestModule::new(fs.root().join(format!("{}", id)));

    Vc::upcast(vc)
}

async fn from_module(module: Vc<Box<dyn Module>>) -> Result<usize> {
    let module: Vc<TestModule> = Vc::try_resolve_downcast_type(module).await?.unwrap();
    let path = module.await?.path.await?;
    path.to_string()
        .split('/')
        .last()
        .unwrap()
        .parse()
        .map_err(Into::into)
}

#[turbo_tasks::value_impl]
impl DepGraph for TestDepGraph {
    #[turbo_tasks::function]
    async fn deps(&self, module: Vc<Box<dyn Module>>) -> Result<Vc<Vec<Vc<Box<dyn Module>>>>> {
        let module = from_module(module).await?;

        Ok(Vc::cell(
            self.deps
                .get(&module)
                .map(|deps| {
                    deps.iter()
                        .map(|(id, _)| Vc::upcast(to_module(self.fs, *id)))
                        .collect()
                })
                .unwrap_or_default(),
        ))
    }

    #[turbo_tasks::function]
    async fn depandants(
        &self,
        module: Vc<Box<dyn Module>>,
    ) -> Result<Vc<Vec<Vc<Box<dyn Module>>>>> {
        let module = from_module(module).await?;

        Ok(Vc::cell(
            self.dependants
                .get(&module)
                .map(|deps| {
                    deps.iter()
                        .map(|&id| Vc::upcast(to_module(self.fs, id)))
                        .collect()
                })
                .unwrap_or_default(),
        ))
    }

    #[turbo_tasks::function]
    async fn get_edge(
        &self,
        from: Vc<Box<dyn Module>>,
        to: Vc<Box<dyn Module>>,
    ) -> Result<Vc<EdgeData>> {
        let from = from_module(from).await?;
        let to = from_module(to).await?;

        let is_lazy = self.lazy.contains(&(from, to));

        Ok(EdgeData { is_lazy }.cell())
    }
}

#[turbo_tasks::value]
struct TestModule {
    path: Vc<FileSystemPath>,
}

#[turbo_tasks::value_impl]
impl TestModule {
    #[turbo_tasks::function]
    fn new(path: Vc<FileSystemPath>) -> Vc<Self> {
        Self { path }.cell()
    }
}

#[turbo_tasks::value_impl]
impl Asset for TestModule {
    #[turbo_tasks::function]
    fn content(self: Vc<Self>) -> Vc<AssetContent> {
        AssetContent::File(FileContent::NotFound.cell()).cell()
    }
}

#[turbo_tasks::value_impl]
impl Module for TestModule {
    #[turbo_tasks::function]
    fn ident(&self) -> Vc<AssetIdent> {
        AssetIdent::from_path(self.path)
    }
}
