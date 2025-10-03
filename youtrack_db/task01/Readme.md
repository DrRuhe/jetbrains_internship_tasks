# YouTrackDB Internship Task 01
In this crate, I will solve this task:

> Please implement a thread-safe version of a sorted in-memory tree using a data structure of your preference. Do not use the existing implementation of data structures. Implement your own instead. Solution that delegates execution to implementing data structures from libraries will be rejected.
>
> Implement only get and put methods and nothing more.
>
> Keys and values of this tree are byte[] arrays.
>
> Please explain why you have chosen the given data structure and publish your code on GitHub for review.

# Technical considerations:
I will implement this in Rust, as this is the language I am most familiar with.

Assuming we name the type `TSIMTree` (**T**hread-**S**afe **I**n-**M**emory Tree).

- keys and values are byte[] arrays
- sorted
- `get` and `put` methods
- thread-safe

This raises the following constraints:
- operations on `TSIMTree` must work on non-mutable references (due to borrowing rules of rust)
- `TSIMTree` must implement `Sync` (and `&TSIMTree` must implement `Send`)
- a constructor method is also required
- the byte arrays can be different size.

I note the following additional design decisions:
- The tree should own the data
- `get` does not delete from the tree.

For Thread Safety, there are multiple strategies
1. Place one Read/Write Mutex on the entire tree
  - on low contention this should have best performance, as only a single lock has to be acquired.
  - on high write workloads, the single mutex becomes a bottleneck.

2. Placing Read-Write Locks on the each tree node:
  - only acquire a Write Lock on nodes that HAVE to be modified during a put.
  - allows paralell insertion when puts modify different nodes
  - allows `get` under nodes that are not being modified by `put`

- I choose to place one Read-Write Lock at the root of the tree, as high-contention is not explicitly stated as a target workload.

Therefore I implement these methods with the given signatures:
- `TSIMTree::new()->TSIMTree` a Constructor
- `TSIMTree::put(k: Deref<[u8]>,v: Vec<u8>)`
  - accepts any key type that can represent a byte array (`Vec<u8>`,`&[u8]`,...)
  - since the tree should own the data, the value is passed as an owned `Vec<u8>`, ensuring the put method does not have to clone the value.
- `TSIMTree::get(k: Deref<[u8]>)->Option<Vec<u8>> `
  - accepts any key type that can represent a byte array (`Vec<u8>`,`&[u8]`,...)
  - the value must be cloned, as a reference could be invalidated by concurrent put calls.


The internals of a tree node are like this:
- the tree has `TREE_RADIX`-ary nodes.
- each node stores key segments. The key segments are ordered inside a node.
- each key segment has an associated child that points to:
  -  a value, in the case where this key is directly part of the tree
  -  another node, which stores values geq than the segment.
- the child of a node that is referenced by a key_segment contains a subtree that stores values with keys greater or equal to the


## Testing Strategy
I implement a small suite of unit tests and also rely on proptests, which uncover edge cases I have yet to handle.

## Problems:
The implementation still has these fundamental issues:

- invalid keys can return values
  - keys that should not actually exist can resolve to values due to the way a key in the domain of a child will search in the child without advancing the key, but therefore a carefully constructed key will resolve to the same value.

- tree balancing & performance
  - no rebalancing operations are implemented, so the tree will stay unbalanced, hurting performance.

## Learnings
Taking this implementation challenge was interesting. Having intentionally stayed clear of researching best practices for implementing in-memory trees I have re-discovered certain patterns that work and others that do not.
