package state

// 新增转换函数
func (self *lkState) ToList(idx int) *lkList {
    val := self.stack.get(idx)
    if list, ok := val.(*lkList); ok {
        return list
    }
    return nil
}

func (self *lkState) ToMap(idx int) *lkMap {
    val := self.stack.get(idx)
    if m, ok := val.(*lkMap); ok {
        return m
    }
    return nil
}

// 列表到映射的转换
func listToMap(list *lkList) *lkMap {
    m := newLkMap(list.len())
    for i := 0; i < list.len(); i++ {
        m.put(int64(i), list.get(int64(i)))
    }
    return m
}

// 映射到列表的转换（仅对数字键）
func mapToList(m *lkMap) *lkList {
    maxIdx := int64(-1)
    
    // 找到最大的整数键
    for k := range m._map {
        if idx, ok := k.(int64); ok && idx >= 0 {
            if idx > maxIdx {
                maxIdx = idx
            }
        }
    }
    
    if maxIdx < 0 {
        return newLkList(0)
    }
    
    list := newLkList(int(maxIdx + 1))
    for i := int64(0); i <= maxIdx; i++ {
        list.set(i, m.get(i))
    }
    
    return list
}