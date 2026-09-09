/* Includes
 * - System */
#include <ds/list.h>
#include <system/heap.h>
#include <system/log.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

/* ListLockAcquire / ListLockRelease
 * Only LIST_SAFE lists lock. An unsafe list used from one thread should
 * not pay for a lock it does not need, and a safe list must use the Irq
 * variant because the garbage collector signals from interrupt context. */
static void ListLockAcquire(List_t *List)
{
    if (List->Attributes & LIST_SAFE) {
        SpinlockAcquireIrq(&List->Lock);
    }
}

static void ListLockRelease(List_t *List)
{
    if (List->Attributes & LIST_SAFE) {
        SpinlockReleaseIrq(&List->Lock);
    }
}

/* ListKeysMatch */
static int ListKeysMatch(KeyType_t Type, DataKey_t A, DataKey_t B)
{
    if (Type == KeyPointer) {
        return (A.Pointer == B.Pointer) ? 1 : 0;
    }
    return (A.Value == B.Value) ? 1 : 0;
}

/* ListCreate */
List_t *ListCreate(KeyType_t KeyType, Flags_t Attributes)
{
    List_t *List = (List_t*)kmalloc(sizeof(List_t));

    if (List == NULL) {
        LogFatal("List", "out of memory");
        return NULL;
    }

    memset(List, 0, sizeof(List_t));
    List->KeyType    = KeyType;
    List->Attributes = Attributes;
    List->Head       = NULL;
    List->Tail       = NULL;
    List->Length     = 0;
    SpinlockReset(&List->Lock);
    return List;
}

/* ListDestroy */
void ListDestroy(List_t *List)
{
    ListNode_t *Node;

    if (List == NULL) {
        return;
    }

    Node = List->Head;
    while (Node != NULL) {
        ListNode_t *Next = Node->Link;
        /* The node is freed, the Data is not - the list never owned it,
         * and freeing it here would double-free whoever does. */
        kfree(Node);
        Node = Next;
    }

    kfree(List);
}

/* ListLength */
size_t ListLength(List_t *List)
{
    size_t Length;

    if (List == NULL) {
        return 0;
    }
    ListLockAcquire(List);
    Length = List->Length;
    ListLockRelease(List);
    return Length;
}

/* ListBegin */
ListNode_t *ListBegin(List_t *List)
{
    return (List == NULL) ? NULL : List->Head;
}

/* ListNext */
ListNode_t *ListNext(ListNode_t *Node)
{
    return (Node == NULL) ? NULL : Node->Link;
}

/* ListCreateNode */
ListNode_t *ListCreateNode(DataKey_t Key, void *Data)
{
    ListNode_t *Node = (ListNode_t*)kmalloc(sizeof(ListNode_t));

    if (Node == NULL) {
        LogFatal("List", "out of memory for a node");
        return NULL;
    }

    memset(Node, 0, sizeof(ListNode_t));
    Node->Key  = Key;
    Node->Data = Data;
    Node->Link = NULL;
    Node->Prev = NULL;
    return Node;
}

/* ListDestroyNode */
void ListDestroyNode(ListNode_t *Node)
{
    if (Node != NULL) {
        kfree(Node);
    }
}

/* ListAppend */
OsStatus_t ListAppend(List_t *List, ListNode_t *Node)
{
    if (List == NULL || Node == NULL) {
        return Error;
    }

    ListLockAcquire(List);

    Node->Link = NULL;
    Node->Prev = List->Tail;

    if (List->Tail == NULL) {
        List->Head = Node;
    }
    else {
        List->Tail->Link = Node;
    }
    List->Tail = Node;
    List->Length++;

    ListLockRelease(List);
    return Success;
}

/* ListPrepend */
OsStatus_t ListPrepend(List_t *List, ListNode_t *Node)
{
    if (List == NULL || Node == NULL) {
        return Error;
    }

    ListLockAcquire(List);

    Node->Prev = NULL;
    Node->Link = List->Head;

    if (List->Head == NULL) {
        List->Tail = Node;
    }
    else {
        List->Head->Prev = Node;
    }
    List->Head = Node;
    List->Length++;

    ListLockRelease(List);
    return Success;
}

/* ListPopFront */
ListNode_t *ListPopFront(List_t *List)
{
    ListNode_t *Node;

    if (List == NULL) {
        return NULL;
    }

    ListLockAcquire(List);

    Node = List->Head;
    if (Node != NULL) {
        List->Head = Node->Link;
        if (List->Head == NULL) {
            List->Tail = NULL;
        }
        else {
            List->Head->Prev = NULL;
        }
        Node->Link = NULL;
        Node->Prev = NULL;
        List->Length--;
    }

    ListLockRelease(List);
    return Node;
}

/* ListGetNodeByKey */
ListNode_t *ListGetNodeByKey(List_t *List, DataKey_t Key)
{
    ListNode_t *Node;

    if (List == NULL) {
        return NULL;
    }

    ListLockAcquire(List);

    Node = List->Head;
    while (Node != NULL) {
        if (ListKeysMatch(List->KeyType, Node->Key, Key)) {
            break;
        }
        Node = Node->Link;
    }

    ListLockRelease(List);
    return Node;
}

/* ListUnlinkInternal
 * Caller holds the lock. */
static void ListUnlinkInternal(List_t *List, ListNode_t *Node)
{
    if (Node->Prev == NULL) {
        List->Head = Node->Link;
    }
    else {
        Node->Prev->Link = Node->Link;
    }

    if (Node->Link == NULL) {
        List->Tail = Node->Prev;
    }
    else {
        Node->Link->Prev = Node->Prev;
    }

    Node->Link = NULL;
    Node->Prev = NULL;
    List->Length--;
}

/* ListRemoveByNode */
OsStatus_t ListRemoveByNode(List_t *List, ListNode_t *Node)
{
    if (List == NULL || Node == NULL) {
        return Error;
    }

    ListLockAcquire(List);
    ListUnlinkInternal(List, Node);
    ListLockRelease(List);
    return Success;
}

/* ListRemoveByKey */
OsStatus_t ListRemoveByKey(List_t *List, DataKey_t Key)
{
    ListNode_t *Node;

    if (List == NULL) {
        return Error;
    }

    ListLockAcquire(List);

    Node = List->Head;
    while (Node != NULL) {
        if (ListKeysMatch(List->KeyType, Node->Key, Key)) {
            ListUnlinkInternal(List, Node);
            break;
        }
        Node = Node->Link;
    }

    ListLockRelease(List);

    if (Node == NULL) {
        return Error;
    }

    kfree(Node);
    return Success;
}