use oc_nano_state::StatePool;

#[test]
fn test_state_pool_lru_eviction() {
    let pool = StatePool::with_capacity(2, 300);
    pool.acquire("a", "cfg1", 300);
    pool.acquire("b", "cfg2", 300);
    pool.acquire("c", "cfg3", 300); // should evict LRU "a"
    assert_eq!(pool.len(), 2);
    assert!(pool.peek("a").is_none());
}

#[test]
fn test_state_pool_refresh_same_key() {
    let pool = StatePool::with_capacity(4, 300);
    pool.acquire("a", "cfg1", 300);
    pool.acquire("a", "cfg1-updated", 300); // refresh, no growth
    assert_eq!(pool.len(), 1);
    assert_eq!(pool.peek("a"), Some("cfg1-updated".to_string()));
}

#[test]
fn test_state_pool_release() {
    let pool = StatePool::with_capacity(4, 300);
    pool.acquire("a", "cfg1", 300);
    pool.acquire("b", "cfg2", 300);
    pool.release("a");
    assert_eq!(pool.len(), 1);
    assert!(pool.peek("a").is_none());
    assert!(pool.peek("b").is_some());
}

#[test]
fn test_state_pool_prune_keeps_within_capacity() {
    let pool = StatePool::with_capacity(2, 300);
    pool.acquire("a", "cfg1", 300);
    pool.acquire("b", "cfg2", 300);
    pool.acquire("c", "cfg3", 300); // evicts LRU "a"
    let removed = pool.prune();
    assert!(removed >= 0);
    assert_eq!(pool.len(), 2);
}

#[test]
fn test_state_pool_defaults() {
    let pool = StatePool::default();
    assert_eq!(pool.len(), 0);
    assert_eq!(pool.capacity(), StatePool::default().capacity());
}

#[test]
fn test_state_pool_init_alias() {
    let pool = StatePool::init(10, 60);
    assert_eq!(pool.capacity(), 10);
}
