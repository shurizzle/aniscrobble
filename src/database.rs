use std::{
    mem::ManuallyDrop,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail};
use heed::types::{Bytes, Str};
use serde::{Deserialize, Serialize};

pub type U64 = heed::types::U64<heed::byteorder::LittleEndian>;

#[derive(Clone)]
struct Delayed<T>(Arc<Mutex<Option<T>>>);

impl<T> Delayed<T> {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }

    pub fn get<E>(&self, f: impl FnOnce() -> Result<T, E>) -> Result<&T, E> {
        let mut data = self.0.lock().unwrap();
        if let Some(data) = data.as_ref() {
            return Ok(unsafe { core::mem::transmute::<&T, &T>(data) });
        }
        *data = Some(f()?);
        Ok(unsafe { core::mem::transmute::<&T, &T>(data.as_ref().unwrap_unchecked()) })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct User {
    pub token: String,
    pub id: u64,
}

impl AsRef<User> for User {
    #[inline(always)]
    fn as_ref(&self) -> &User {
        self
    }
}

#[derive(Clone)]
pub struct Database {
    env: heed::Env,
    main: heed::Database<Str, Bytes>,
    data: Delayed<heed::Database<U64, U64>>,
}

impl crate::IsFatal for heed::Error {
    fn is_fatal(&self) -> bool {
        match self {
            heed::Error::EnvAlreadyOpened | heed::Error::Io(_) | heed::Error::Mdb(_) => true,
            heed::Error::Encoding(_) | heed::Error::Decoding(_) => todo!(),
        }
    }
}

#[inline(always)]
fn bincode_deserialize<'a, T>(bytes: &'a [u8]) -> heed::Result<T>
where
    T: serde::de::Deserialize<'a>,
{
    bincode::deserialize(bytes).map_err(|e| heed::Error::Decoding(Box::new(e)))
}

#[inline(always)]
fn bincode_serialize<T>(value: &T) -> heed::Result<Vec<u8>>
where
    T: serde::Serialize + ?Sized,
{
    bincode::serialize(value).map_err(|e| heed::Error::Encoding(Box::new(e)))
}

impl Database {
    pub fn new() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("dev", "shurizzle", "aniscrobble").unwrap();
        let db_file = dirs.cache_dir().join("data.db");
        std::fs::create_dir_all(&db_file).context("cannot open database")?;
        let env = unsafe {
            heed::EnvOpenOptions::new()
                .max_dbs(2)
                .open(&db_file)
                .context("cannot open database")?
        };
        let main: heed::Database<Str, Bytes>;
        {
            let mut wtxn = env.write_txn().context("cannot open database")?;
            main = env
                .create_database(&mut wtxn, None)
                .context("cannot open database")?;
            if let Some(version) = main.get(&wtxn, "version")? {
                let version =
                    bincode::deserialize::<u64>(version).context("invalid database version")?;
                if version != 0 {
                    bail!("invalid database version");
                }
            } else {
                main.put(&mut wtxn, "version", &bincode::serialize(&0u64)?)
                    .context("cannot open database")?;
            }
            wtxn.commit().context("cannot open database")?;
        }
        Ok(Self {
            env,
            main,
            data: Delayed::new(),
        })
    }

    fn data(&self, wtxn: Option<&mut heed::RwTxn>) -> heed::Result<&heed::Database<U64, U64>> {
        if let Some(wtxn) = wtxn {
            self.data
                .get(|| self.env.create_database(wtxn, Some("data")))
        } else {
            self.data.get(|| {
                let mut wtxn = self.env.write_txn()?;
                self.env.create_database(&mut wtxn, Some("data"))
            })
        }
    }

    pub fn login(&self) -> heed::Result<Option<User>> {
        let rtxn = self.env.read_txn()?;
        self.main
            .get(&rtxn, "login")?
            .map(bincode_deserialize::<User>)
            .transpose()
    }

    pub fn set_login(&self, user: impl AsRef<User>) -> heed::Result<()> {
        let mut wtxn = self.env.write_txn()?;
        self.main
            .put(&mut wtxn, "login", &bincode_serialize(user.as_ref())?)?;
        wtxn.commit()?;
        Ok(())
    }

    pub fn delete_login(&self) -> heed::Result<()> {
        let mut wtxn = self.env.write_txn()?;
        self.main.delete(&mut wtxn, "login")?;
        wtxn.commit()?;
        Ok(())
    }

    pub fn scrobble(&self, id: u64, episode: u64) -> heed::Result<()> {
        let mut wtxn = self.env.write_txn()?;
        let data = self.data(Some(&mut wtxn))?;
        if data.get(&wtxn, &id)?.map(|ep| ep < episode).unwrap_or(true) {
            let mut pending = self
                .main
                .get(&wtxn, "pending")?
                .map(bincode_deserialize::<Vec<u64>>)
                .transpose()?
                .unwrap_or_default();
            if let Err(i) = pending.binary_search(&id) {
                pending.insert(i, id);
                self.main
                    .put(&mut wtxn, "pending", &bincode_serialize(&pending)?)?;
            }
            data.put(&mut wtxn, &id, &episode)?;
            wtxn.commit()?;
        }
        Ok(())
    }

    pub fn sync(&self) -> heed::Result<SyncContext> {
        let wtxn = self.env.write_txn()?;
        let pending = self
            .main
            .get(&wtxn, "pending")?
            .map(bincode_deserialize::<Vec<u64>>)
            .transpose()?
            .unwrap_or_default();
        Ok(SyncContext {
            changed: false,
            txn: ManuallyDrop::new(wtxn),
            pending,
            db: self,
            idx: 0,
        })
    }
}

pub struct SyncContext<'a> {
    changed: bool,
    txn: ManuallyDrop<heed::RwTxn<'a>>,
    pending: Vec<u64>,
    db: &'a Database,
    idx: usize,
}

pub struct Anime<'a> {
    ctx: &'a mut SyncContext<'a>,
    id: u64,
    episode: u64,
}

impl std::fmt::Debug for Anime<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Anime")
            .field("id", &self.id)
            .field("episode", &self.episode)
            .finish()
    }
}

impl Drop for Anime<'_> {
    fn drop(&mut self) {
        self.ctx.idx += 1;
    }
}

impl SyncContext<'_> {
    pub fn next(&mut self) -> Option<heed::Result<Anime>> {
        loop {
            let id = *self.pending.get(self.idx)?;
            let episode = self
                .db
                .data(Some(&mut self.txn))
                .and_then(|data| data.get(&self.txn, &id));
            let episode = match episode {
                Ok(e) => e,
                Err(err) => return Some(Err(err)),
            };
            if let Some(episode) = episode {
                return Some(Ok(Anime {
                    ctx: unsafe {
                        core::mem::transmute::<&mut SyncContext<'_>, &mut SyncContext<'_>>(self)
                    },
                    id,
                    episode,
                }));
            }
            self.pending.remove(self.idx);
            self.changed = true;
        }
    }

    pub fn commit(self) -> heed::Result<()> {
        let changed = self.changed;
        let pending = unsafe { std::ptr::read_volatile(&self.pending) };
        let mut txn = unsafe { std::ptr::read_volatile(&*self.txn) };
        let db = unsafe { std::ptr::read_volatile(&self.db) };
        core::mem::forget(self);

        if changed {
            match bincode_serialize(&pending)
                .and_then(|bincode| db.main.put(&mut txn, "pending", &bincode))
            {
                Ok(()) => txn.commit(),
                Err(err) => {
                    _ = txn.commit();
                    Err(err)
                }
            }
        } else {
            txn.commit()
        }
    }
}

impl Drop for SyncContext<'_> {
    fn drop(&mut self) {
        let mut txn = unsafe { std::ptr::read_volatile(&*self.txn) };
        if self.changed {
            if let Ok(pending) = bincode::serialize(&self.pending) {
                _ = self.db.main.put(&mut txn, "pending", &pending);
            }
        }
        _ = txn.commit();
    }
}

impl Anime<'_> {
    #[inline(always)]
    pub fn id(&self) -> u64 {
        self.id
    }

    #[inline(always)]
    pub fn episode(&self) -> u64 {
        self.episode
    }

    pub fn update(self, episode: u64) -> heed::Result<()> {
        let ctx = unsafe { core::ptr::read_volatile(&self.ctx) };
        let id = self.id;
        let old_episode = self.episode;
        std::mem::forget(self);

        let pid = ctx.pending.binary_search(&id).expect("pending episode");
        debug_assert!(episode >= old_episode);
        if episode > old_episode {
            ctx.db
                .data(Some(&mut ctx.txn))?
                .put(&mut ctx.txn, &id, &episode)?;
        }
        ctx.pending.remove(pid);
        ctx.changed = true;
        Ok(())
    }
}
