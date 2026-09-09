#ifndef __LIST_H__
#define __LIST_H__

#include <defs.h>
#include <stddef.h>
#include <system/spinlock.h>

/* Key types a node can be looked up by. The reference supports strings
 * too; there is no string type in this kernel yet, so integers and raw
 * pointers cover everything that needs looking up today. */
typedef enum {
    KeyInteger,
    KeyPointer
} KeyType_t;

typedef union _DataKey {
    int         Value;
    void       *Pointer;
} DataKey_t;

typedef struct _ListNode {
    DataKey_t           Key;
    void               *Data;
    struct _ListNode   *Link;   /* next     */
    struct _ListNode   *Prev;   /* previous */
} ListNode_t;

typedef struct _List {
    KeyType_t           KeyType;
    Flags_t             Attributes;
    ListNode_t         *Head;
    ListNode_t         *Tail;
    size_t              Length;
    Spinlock_t          Lock;
} List_t;

/* LIST_SAFE takes the lock on every operation. Use it for any list
 * touched from more than one thread, or from interrupt context. */
#define LIST_NORMAL                 0x0
#define LIST_SAFE                   0x1

/* Iteration helpers. The _nolink form is for loops that remove the
 * current node - advancing after a free would read released memory. */
#define foreach(i, List) \
    ListNode_t *i; for (i = ListBegin(List); i != NULL; i = ListNext(i))
#define _foreach(i, List) \
    for (i = ListBegin(List); i != NULL; i = ListNext(i))
#define _foreach_nolink(i, List) \
    for (i = ListBegin(List); i != NULL; )

#ifdef __cplusplus
extern "C" {
#endif

/* ListCreate / ListDestroy
 * ListDestroy frees the nodes, not the Data they point at - the list
 * does not own what it holds. */
List_t *ListCreate(KeyType_t KeyType, Flags_t Attributes);
void ListDestroy(List_t *List);

/* ListLength / ListBegin / ListNext */
size_t ListLength(List_t *List);
ListNode_t *ListBegin(List_t *List);
ListNode_t *ListNext(ListNode_t *Node);

/* ListCreateNode
 * Allocates a node. Returns NULL when out of memory. */
ListNode_t *ListCreateNode(DataKey_t Key, void *Data);

/* ListDestroyNode
 * Frees a node that is not in a list. */
void ListDestroyNode(ListNode_t *Node);

/* ListAppend / ListPrepend
 * Insert an already-created node. */
OsStatus_t ListAppend(List_t *List, ListNode_t *Node);
OsStatus_t ListPrepend(List_t *List, ListNode_t *Node);

/* ListPopFront
 * Removes and returns the first node, or NULL. This is the queue
 * primitive the garbage collector is built on. */
ListNode_t *ListPopFront(List_t *List);

/* ListGetNodeByKey
 * First node whose key matches, or NULL. */
ListNode_t *ListGetNodeByKey(List_t *List, DataKey_t Key);

/* ListRemoveByNode / ListRemoveByKey
 * Unlink. RemoveByNode does not free the node; RemoveByKey does. */
OsStatus_t ListRemoveByNode(List_t *List, ListNode_t *Node);
OsStatus_t ListRemoveByKey(List_t *List, DataKey_t Key);

#ifdef __cplusplus
}
#endif

#endif /* __LIST_H__ */