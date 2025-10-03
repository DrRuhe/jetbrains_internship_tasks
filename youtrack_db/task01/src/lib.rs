use std::array;
use std::fmt::Debug;
use std::sync::RwLock;
use std::cmp::Ordering;

const CACHE_LINE_SIZE: usize = 128;
const TREE_RADIX: usize = 16;

#[derive(Debug)]
pub struct TSIMTree {
    root: RwLock<TSIMTreeNode>,
}

impl TSIMTree {
    pub fn new() -> TSIMTree {
        TSIMTree {
            root: RwLock::new(TSIMTreeNode::empty()),
        }
    }

    pub fn put<K>(&self, k: K, v: Vec<u8>)
    where
        K: AsRef<[u8]>,
    {
        let mut key: &[u8] = k.as_ref();
        let mut node_guard = self
            .root
            .write()
            .expect("Must be able to acquire write lock");
        let mut node = &mut *node_guard;

        loop {
            match node.resolve_child(key) {
                ResolvedChild::Smallest => {
                    if (node.children_count as usize) < TREE_RADIX {
                        node.insert_child(0, key, TSIMTreeNodeChild::with_mapping(key, v));
                        return;
                    }

                    // There is no space in this node, so we must replace the key_segment in this node with the new segment.
                    // But what do we do with the old key? We dont know which
                    let old_key_fragment = node.get_segment(0).to_owned();
                    let child = node.children[0]
                        .as_mut()
                        .expect("node.children[0] must be Some(..)");
                    child.pushdown_children_under_key(&old_key_fragment);

                    let (new_key_fragment, remaining_key) = key.split_at(old_key_fragment.len());

                    node.set_segment(0, new_key_fragment);
                    let child = node.children[0].as_mut();

                    let TSIMTreeNodeChild::Node(n) =
                        child.expect("node.children[0] must be Some(..)")
                    else {
                        panic!("remaining_key is not empty, so new_node must be TSIMTreeNodeChild::Node(..)")
                    };
                    node = n;
                    key = remaining_key;
                }

                ResolvedChild::ExactMatch(segment, remaining_key) => {
                    let borrowed_child = node.children[segment].as_mut();
                    let child = borrowed_child.expect("children[child_idx] must be Some(..)");
                    match child {
                        TSIMTreeNodeChild::Value(old_val) if remaining_key.is_empty() => {
                            *old_val = v;
                            return;
                        }
                        TSIMTreeNodeChild::Value(old_val) => {
                            // The existing value is stored under a prefix of the new value.
                            // We must replace the value with a new Node that contains the old value AND the new one.

                            let mut new_node = TSIMTreeNodeChild::with_mapping(remaining_key, v);
                            let TSIMTreeNodeChild::Node(n) = &mut new_node else {
                                panic!("remaining_key is not empty, so new_node must be TSIMTreeNodeChild::Node(..)")
                            };
                            n.insert_child(0, &[], TSIMTreeNodeChild::Value(old_val.to_owned()));
                            *child = new_node;
                            return;
                        }

                        TSIMTreeNodeChild::Node(new_node) => {
                            node = new_node;
                            key = remaining_key;
                        }
                    }
                }
                ResolvedChild::InDomainOf(segment) => {
                    let borrowed_child = node.children[segment].as_mut();
                    let child = borrowed_child.expect("children[child_idx] must be Some(..)");
                    match child {
                        TSIMTreeNodeChild::Value(old_val) => {
                            // We must insert a new node to house old value together with the new value.

                            let mut new_node = TSIMTreeNodeChild::with_mapping(key, v);
                            let TSIMTreeNodeChild::Node(n) = &mut new_node else {
                                panic!("remaining_key is not empty, so new_node must be TSIMTreeNodeChild::Node(..)")
                            };
                            n.insert_child(0, &[], TSIMTreeNodeChild::Value(old_val.to_owned()));
                            *child = new_node;
                            return;
                        }
                        TSIMTreeNodeChild::Node(new_node) => {
                            node = new_node;
                        }
                    }
                }
            };
        }
    }

    pub fn get<'s, K>(&'s self, k: K) -> Option<Vec<u8>>
    where
        K: AsRef<[u8]>,
    {
        let mut key: &[u8] = k.as_ref();
        let node_guard = self.root.read().expect("Must be able to acquire read lock");
        let mut node = &*node_guard;
        loop {
            match node.resolve_child(key) {
                ResolvedChild::Smallest => return None,
                ResolvedChild::ExactMatch(segment, remaining_key) => {
                    match &node.children[segment]
                        .as_ref()
                        .expect("children[child_idx] must be Some(..)")
                    {
                        TSIMTreeNodeChild::Value(v) => {
                            if remaining_key.is_empty() {
                                return Some(v.clone());
                            } else {
                                return None;
                            }
                        }
                        TSIMTreeNodeChild::Node(new_node) => {
                            assert!(node != new_node.as_ref());
                            node = new_node;
                            key = remaining_key;
                        }
                    }
                }
                ResolvedChild::InDomainOf(segment) => {
                    let TSIMTreeNodeChild::Node(new_node) = &node.children[segment]
                        .as_ref()
                        .expect("children[segment] must be Some(..)")
                    else {
                        // If the key is in the domain of a Value child, the actual key does not exist in the tree
                        return None;
                    };
                    assert!(node != new_node.as_ref());
                    node = new_node;
                }
            };
        }
    }
}

const KEY_SEGMENT_SIZE: usize = CACHE_LINE_SIZE / TREE_RADIX;

#[derive(PartialEq, Eq, Clone)]
#[repr(C, align(128))]
struct TSIMTreeNode {
    key_segments: [[u8; KEY_SEGMENT_SIZE]; TREE_RADIX],
    children: [Option<TSIMTreeNodeChild>; TREE_RADIX],
    children_count: u8,
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum TSIMTreeNodeChild {
    Node(Box<TSIMTreeNode>),
    Value(Vec<u8>),
}

#[derive(Debug)]
#[allow(dead_code)]
enum TSIMTreeFault {
    InvalidSegment {
        len: u8,
    },
    ChildIsNone {
        child_idx: usize,
        children_count: u8,
    },
}

#[derive(Debug, PartialEq, Eq)]
/// Encodes the location of a child in a node.
enum ResolvedChild<'k> {
    /// The queried key is outside the domain of any existing child.
    Smallest,
    /// The queried key exactly matches the key segment at this index.
    /// The remaining key fragment is returned as well.
    ExactMatch(usize, &'k [u8]),
    /// The queried key does not match directly but is in the domain of this child
    /// In this case, no remaining key fragment is returned, the previous key must be reused in the query.
    InDomainOf(usize),
}

const MAX_STORED_KEY_SEGMENT_SIZE: usize = KEY_SEGMENT_SIZE - 1;
impl TSIMTreeNode {
    fn empty() -> TSIMTreeNode {
        TSIMTreeNode {
            key_segments: [[0; KEY_SEGMENT_SIZE]; TREE_RADIX],
            children: array::from_fn(|_| None),
            children_count: 0,
        }
    }

    /// Stores a fragment of a key at the given segment index.
    fn set_segment(&mut self, segment_idx: usize, key_fragment: &[u8]) {
        assert!(segment_idx < TREE_RADIX);

        let key_len = key_fragment.len();
        assert!(key_len <= MAX_STORED_KEY_SEGMENT_SIZE);

        let (length, buffer) = self.key_segments[segment_idx].split_at_mut(1);
        length[0] = key_len as u8;
        let (segment_buf, _unused) = buffer.split_at_mut(key_len);
        segment_buf.copy_from_slice(key_fragment);
    }

    fn get_segment(&self, segment_idx: usize) -> &[u8] {
        assert!(segment_idx < TREE_RADIX);
        TSIMTreeNode::stored_segment(&self.key_segments[segment_idx])
            .expect("Segment must be valid!")
    }

    /// The buffer for the segments contains length bytes and subsequently the segment.
    /// This function reads the length byte and returns a reference to part of the buffer that represent the segment.
    fn stored_segment<'s>(segment: &'s [u8]) -> Result<&'s [u8], TSIMTreeFault> {
        let (len_buffer, segment_buffer) = segment.split_at(1);
        let stored_segment_length = len_buffer[0];

        if stored_segment_length as usize > MAX_STORED_KEY_SEGMENT_SIZE {
            return Err(TSIMTreeFault::InvalidSegment {
                len: stored_segment_length,
            });
        }

        let (stored_segment, _unused_segment) =
            segment_buffer.split_at(stored_segment_length as usize);
        Ok(stored_segment)
    }

    /// Compares two key segments and returns an ordering for the compared segment and a remaining key segment
    fn compare_key_segment<'k>(segment: &[u8], key: &'k [u8]) -> (Ordering, &'k [u8]) {
        let stored_segment = TSIMTreeNode::stored_segment(segment).expect("segment must be valid!");

        let key_segment_length = key.len().min(stored_segment.len());
        let (expected_key_segment, remaining_key) = key.split_at(key_segment_length);
        let ordering = expected_key_segment.cmp(stored_segment);

        (ordering, remaining_key)
    }

    /// Use binary search to figure out under what child the key could be located.
    fn resolve_child<'k>(&self, key: &'k [u8]) -> ResolvedChild<'k> {
        let mut left_segment_idx = 0;
        let mut right_segment_idx = self.children_count as usize;

        if self.children_count == 0 {
            return ResolvedChild::Smallest;
        }
        assert!(right_segment_idx as usize <= TREE_RADIX);
        // Binary search in the segments for the next hop:
        while left_segment_idx < right_segment_idx {
            let segment = left_segment_idx + (right_segment_idx - left_segment_idx) / 2;

            match TSIMTreeNode::compare_key_segment(&self.key_segments[segment], key) {
                (Ordering::Equal, remaining_key) => {
                    return ResolvedChild::ExactMatch(segment, remaining_key)
                }
                (Ordering::Greater, _) if (left_segment_idx + 1 == right_segment_idx) => {
                    return ResolvedChild::InDomainOf(segment)
                }
                (Ordering::Greater, _) => left_segment_idx = segment,
                (Ordering::Less, _) => right_segment_idx = segment,
            }
        }
        ResolvedChild::Smallest
    }

    fn insert_child(&mut self, idx: usize, key_fragment: &[u8], child: TSIMTreeNodeChild) {
        assert!(
            (self.children_count as usize) < TREE_RADIX,
            "Cannot insert into full node"
        );

        // Copy over all the key segments
        if idx <= self.children_count as usize {
            let (_unchanged, children) = self.children.split_at_mut(idx);
            let (_unchanged, key_segments) = self.key_segments.split_at_mut(idx);
            children.rotate_right(1);
            key_segments.rotate_right(1);
        }

        self.set_segment(idx, key_fragment);
        self.children[idx] = Some(child);
    }
}

impl TSIMTreeNodeChild {
    /// Creates a subtree to store the value at the given key.
    fn with_mapping(key: &[u8], value: Vec<u8>) -> TSIMTreeNodeChild {
        key.chunks(MAX_STORED_KEY_SEGMENT_SIZE)
            .map(|key_fragment| {
                let mut node = TSIMTreeNode {
                    key_segments: [[0; KEY_SEGMENT_SIZE]; TREE_RADIX],
                    children: array::from_fn(|_| None),
                    children_count: 1,
                };

                node.set_segment(0, key_fragment);

                TSIMTreeNodeChild::Node(Box::new(node))
            })
            .rev()
            .fold(TSIMTreeNodeChild::Value(value), |child, mut node| {
                let TSIMTreeNodeChild::Node(n) = &mut node else {
                    panic!("Element of the iterator are initialized as Node variants of the enum");
                };
                n.children[0] = Some(child);
                return node;
            })
    }

    /// Will modify the current node, so that the node is effectively pushed one layer down.
    /// the old_key_fragment should have pointed to this self node.
    /// This effectively creates new space at the self node.
    fn pushdown_children_under_key(&mut self, old_key_fragment: &[u8]) {
        let mut node = TSIMTreeNode {
            key_segments: [[0; KEY_SEGMENT_SIZE]; TREE_RADIX],
            children: array::from_fn(|_| None),
            children_count: 1,
        };
        node.set_segment(0, old_key_fragment);

        let mut node_child = TSIMTreeNodeChild::Node(Box::new(node));

        std::mem::swap(self, &mut node_child);

        let TSIMTreeNodeChild::Node(self_node) = self else {
            panic!("self was just set to TSIMTreeNodeChild::Node(...)");
        };

        self_node.children[0] = Some(node_child)
    }
}

impl Debug for TSIMTreeNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut builder = &mut f.debug_map();

        for child_idx in 0..self.children_count as usize {
            let key_builder =
                match TSIMTreeNode::stored_segment(self.key_segments[child_idx].as_slice()) {
                    Ok(segment) => builder.key(&format!("{segment:X?}")),
                    Err(e) => builder.key(&e),
                };

            builder = match &self.children[child_idx] {
                Some(TSIMTreeNodeChild::Node(node)) => key_builder.value(&node),
                Some(TSIMTreeNodeChild::Value(value)) => key_builder.value(&format!("{value:X?}")),
                None => key_builder.value(&TSIMTreeFault::ChildIsNone {
                    child_idx: child_idx,
                    children_count: self.children_count,
                }),
            };
        }

        builder.finish()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_comparison_behavior() {
        assert_eq!(b"abc".as_slice().cmp(b"abc".as_slice()), Ordering::Equal);
        assert_eq!(b"ab".as_slice().cmp(b"abc".as_slice()), Ordering::Less);
        assert_eq!(b"abc".as_slice().cmp(b"ab".as_slice()), Ordering::Greater);
    }

    #[test]
    fn test_pretty_printing() {
        let mut tree = TSIMTree::new();
        tree.put(b"key1", b"val1".into());

        dbg!(tree);
    }

    #[test]
    fn test_node_resolving() {
        println!("Initializing Node");
        let mut node = TSIMTreeNode {
            key_segments: Default::default(),
            children: array::from_fn(|i| Some(TSIMTreeNodeChild::Value(vec![i as u8]))),
            children_count: TREE_RADIX as u8,
        };

        let first_key = 1 as u8;
        let last_key = TREE_RADIX as u8 + 1;

        assert_eq!((first_key..last_key).len(), TREE_RADIX);

        println!("Initializing Segments");

        for (segment, key) in (0..TREE_RADIX).zip(first_key..last_key) {
            let buf = vec![key];
            node.set_segment(segment, buf.as_slice());
        }

        println!("Retrieving Children");

        let first_child = node.children[0].clone().expect("All children are Some");
        let last_child = node.children[TREE_RADIX - 1]
            .clone()
            .expect("All children are Some");

        let v = vec![];
        let empty_slice: &[u8] = v.as_slice();

        dbg!(&node);

        // Since the keys are stored with +1 offset, if we search for 0, there is None, if we search for 1 we get the first element, at idx 0.
        assert_eq!(
            node.resolve_child(vec![first_key - 1].as_slice()),
            ResolvedChild::Smallest
        );

        assert_eq!(
            node.resolve_child(vec![first_key].as_slice()),
            ResolvedChild::ExactMatch(0, empty_slice)
        );
        // looking for the last key and beyond, we return the last child
        assert_eq!(
            node.resolve_child(dbg![vec![last_key - 1].as_slice()]),
            ResolvedChild::ExactMatch(TREE_RADIX - 1, empty_slice)
        );
        assert_eq!(
            node.resolve_child(vec![last_key].as_slice()),
            ResolvedChild::InDomainOf(TREE_RADIX - 1)
        );
    }

    #[test]
    fn test_basic_insert_and_get() {
        let tree = TSIMTree::new();
        dbg!(&tree).put(b"key1", b"val1".into());
        dbg!(&tree).put(b"key2", b"val2".into());

        assert_eq!(dbg!(&tree).get(b"key1"), Some(b"val1".to_vec()));
        assert_eq!(tree.get(b"key2"), Some(b"val2".to_vec()));
    }

    #[test]
    fn test_overwrite_value() {
        let mut tree = TSIMTree::new();
        tree.put(b"key", b"first".into());
        tree.put(b"key", b"second".into());

        assert_eq!(tree.get(b"key"), Some(b"second".to_vec()));
    }

    #[test]
    fn test_missing_key() {
        let mut tree = TSIMTree::new();
        tree.put(b"key", b"value".into());

        assert_eq!(tree.get(b"other"), None);
    }

    #[test]
    fn test_multiple_sizes() {
        let mut tree = TSIMTree::new();
        tree.put(b"k", b"1".into());
        tree.put(b"key", b"v".into());
        tree.put(b"", b"empty".into());
        tree.put(b"a", b"A".into());

        assert_eq!(tree.get(b""), Some(b"empty".to_vec()));
        assert_eq!(tree.get(b"k"), Some(b"1".to_vec()));
        assert_eq!(tree.get(b"a"), Some(b"A".to_vec()));
        assert_eq!(tree.get(b"key"), Some(b"v".to_vec()));
    }

    #[test]
    fn test_key_byte_equality() {
        let mut tree = TSIMTree::new();

        // Two keys with same content but different Vec allocations
        let k1: Vec<u8> = b"identical".into();
        let k2: Vec<u8> = b"identical".into();
        let v: Vec<u8> = b"value".into();

        tree.put(&k1, v.clone());
        assert_eq!(tree.get(&k2), Some(v));
    }

    // #[test]
    // fn test_concurrent_inserts_and_gets() {
    //     let tree = Arc::new(TSIMTree::new());
    //     let num_threads = 8;
    //     let num_keys = 100;

    //     // Spawn threads for concurrent puts
    //     let mut handles = vec![];
    //     for tid in 0..num_threads {
    //         let t_clone = Arc::clone(&tree);
    //         handles.push(thread::spawn(move || {
    //             for i in 0..num_keys {
    //                 let k = format!("k{}_{}", tid, i).into_bytes();
    //                 let v = format!("v{}_{}", tid, i).into_bytes();
    //                 t_clone.put(k, v);
    //             }
    //         }));
    //     }
    //     // Wait for all insertions
    //     for h in handles {
    //         h.join().expect("thread panicked");
    //     }

    //     // Concurrent gets
    //     let mut handles = vec![];
    //     for tid in 0..num_threads {
    //         let t_clone = Arc::clone(&tree);
    //         handles.push(thread::spawn(move || {
    //             for i in 0..num_keys {
    //                 let k = format!("k{}_{}", tid, i).into_bytes();
    //                 let expected = format!("v{}_{}", tid, i).into_bytes();
    //                 assert_eq!(t_clone.get(&k), Some(expected.as_slice()));
    //             }
    //         }));
    //     }
    //     for h in handles {
    //         h.join().expect("thread panicked");
    //     }
    // }

    #[test]
    fn test_keys_with_null_bytes() {
        let mut tree = TSIMTree::new();
        tree.put(&b"key\0with\0nulls"[..], b"value".into());
        assert_eq!(tree.get(&b"key\0with\0nulls"[..]), Some(b"value".to_vec()));
    }

    use proptest::prelude::*;
    use std::collections::HashMap;

    proptest! {
        #[test]
        fn tsimtree_behaves_like_hashmap(
            ops in proptest::collection::vec((proptest::collection::vec(any::<u8>(), 0..32), proptest::collection::vec(any::<u8>(), 0..32)), 1..32)
        ) {
            let mut ref_map = HashMap::new();
            let mut tree = TSIMTree::new();

            for (k, v) in &ops {
                ref_map.insert(k.clone(), v.clone());
                tree.put(k.clone(), v.clone());
            }

            // Assert that all keys in the reference HashMap yield the same results
            for (k, v) in &ref_map {
                let tree_value = tree.get(k.clone());
                prop_assert!(tree_value.is_some());
                prop_assert_eq!(tree_value.unwrap(), v.as_slice());
            }

            // Optionally: check that querying missing keys yields None
            let absent_key = vec![42, 13, 7];
            if !ref_map.contains_key(&absent_key) {
                prop_assert_eq!(tree.get(absent_key), None);
            }
        }
    }
}
