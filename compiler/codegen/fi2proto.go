package codegen

import . "git.lolli.tech/lollipopkit/lk/binchunk"

func toProto(fi *funcInfo) *Prototype {
	proto := &Prototype{
		LineDefined:     uint32(fi.line),
		LastLineDefined: uint32(fi.lastLine),
		NumParams:       byte(fi.numParams),
		MaxStackSize:    byte(fi.maxRegs),
		Code:            fi.insts,
		Constants:       getConstants(fi),
		Upvalues:        getUpvalues(fi),
		Protos:          toProtos(fi.subFuncs),
		LineInfo:        fi.lineNums,
		LocVars:         getLocVars(fi),
		UpvalueNames:    getUpvalueNames(fi),
	}

	if fi.line == 0 {
		proto.LastLineDefined = 0
	}
	if proto.MaxStackSize < 2 {
		proto.MaxStackSize = 2 // todo
	}
	if fi.isVararg {
		proto.IsVararg = 1 // todo
	}

	return proto
}

func toProtos(fis []*funcInfo) []*Prototype {
	protos := make([]*Prototype, len(fis))
	for i := range fis {
		protos[i] = toProto(fis[i])
	}
	return protos
}

func getConstants(fi *funcInfo) []interface{} {
	consts := make([]interface{}, len(fi.constants))
	for k := range fi.constants {
		consts[fi.constants[k]] = k
	}
	return consts
}

func getLocVars(fi *funcInfo) []LocVar {
	locVars := make([]LocVar, len(fi.locVars))
	for i := range fi.locVars {
		locVars[i] = LocVar{
			VarName: fi.locVars[i].name,
			StartPC: uint32(fi.locVars[i].startPC),
			EndPC:   uint32(fi.locVars[i].endPC),
		}
	}
	return locVars
}

func getUpvalues(fi *funcInfo) []Upvalue {
	upvals := make([]Upvalue, len(fi.upvalues))
	for i := range fi.upvalues {
		if fi.upvalues[i].locVarSlot >= 0 { // instack
			upvals[fi.upvalues[i].index] = Upvalue{1, byte(fi.upvalues[i].locVarSlot)}
		} else {
			upvals[fi.upvalues[i].index] = Upvalue{0, byte(fi.upvalues[i].upvalIndex)}
		}
	}
	return upvals
}

func getUpvalueNames(fi *funcInfo) []string {
	names := make([]string, len(fi.upvalues))
	for name := range fi.upvalues {
		names[fi.upvalues[name].index] = name
	}
	return names
}
