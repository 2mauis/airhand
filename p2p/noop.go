package p2p

// polarity 是尚未实现的桩的空操作极性（docs/specs/tailcat-p2p.md「先红验收测试清单」#99）。
// 桩不实现任何行为，只返回可用的空操作对象，先红用例因断言不满足而失败：
//   - strict：全部丢弃、闸门恒关、计数为 0（未设 P2P_TEST_NOOP 时的默认值，即常规作业）；
//   - lenient：什么都不做、闸门打开后永不关闭、数据报照常放行。
//
// 由测试的 TestMain 按 P2P_TEST_NOOP=lenient|strict 设置；Phase 1 实现之后删除。
type polarity int

const (
	strict polarity = iota
	lenient
)

var noop = strict
