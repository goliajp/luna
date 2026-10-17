// CODEGEN: gctrace libgen.sh (an instrumented PUC's luaL_openlibs)
//! What PUC's `luaL_openlibs` does to the parts of each library
//! table it names, by dialect, from the table's making on: its
//! resizes other than a rehash's and the string keys it puts in, in
//! order (`R` and two u16 sizes, or `K`, a length and the key).

static BLOB_51: &[u8] = b"K\x07__indexK\x05closeK\x05flushK\x05linesK\x04readK\x04seekK\x07setvbufK\x05writeK\x04__gcK\x0a__tostringR\x00\x00\x02\x00K\x02_GK\x06assertK\x0ecollectgarbageK\x06dofileK\x05errorK\x06gcinfoK\x07getfenvK\x0cgetmetatableK\x08loadfileK\x04loadK\x0aloadstringK\x04nextK\x05pcallK\x05printK\x08rawequalK\x06rawgetK\x06rawsetK\x06selectK\x07setfenvK\x0csetmetatableK\x08tonumberK\x08tostringK\x04typeK\x06unpackK\x06xpcallK\x08_VERSIONK\x06ipairsK\x05pairsK\x08newproxyK\x09coroutineK\x07packageK\x06moduleK\x07requireK\x05tableK\x02ioK\x02osK\x06stringK\x04mathK\x05debugR\x00\x00\x06\x00K\x06createK\x06resumeK\x07runningK\x06statusK\x04wrapK\x05yieldR\x00\x00\x0e\x00K\x05debugK\x07getfenvK\x07gethookK\x07getinfoK\x08getlocalK\x0bgetregistryK\x0cgetmetatableK\x0agetupvalueK\x07setfenvK\x07sethookK\x08setlocalK\x0csetmetatableK\x0asetupvalueK\x09tracebackR\x00\x00\x0b\x00K\x05closeK\x05flushK\x05inputK\x05linesK\x04openK\x06outputK\x05popenK\x04readK\x07tmpfileK\x04typeK\x05writeK\x05stdinK\x06stdoutK\x06stderrR\x00\x00\x1c\x00K\x03absK\x04acosK\x04asinK\x05atan2K\x04atanK\x04ceilK\x04coshK\x03cosK\x03degK\x03expK\x05floorK\x04fmodK\x05frexpK\x05ldexpK\x05log10K\x03logK\x03maxK\x03minK\x04modfK\x03powK\x03radK\x06randomK\x0arandomseedK\x04sinhK\x03sinK\x04sqrtK\x04tanhK\x03tanK\x02piK\x04hugeK\x03modR\x00\x00\x0b\x00K\x05clockK\x04dateK\x08difftimeK\x07executeK\x04exitK\x06getenvK\x06removeK\x06renameK\x09setlocaleK\x04timeK\x07tmpnameR\x00\x00\x02\x00K\x07loadlibK\x06seeallK\x07loadersK\x04pathK\x05cpathK\x06configK\x06loadedK\x07preloadR\x04\x00\x00\x00R\x00\x00\x02\x00K\x07_LOADEDK\x08_LOADLIBK\x05FILE*R\x00\x00\x01\x00K\x02_GK\x09coroutineK\x07packageK\x05tableK\x02ioK\x02osK\x06stringK\x04mathK\x05debugK\x04__gcR\x00\x00\x0f\x00K\x04byteK\x04charK\x04dumpK\x04findK\x06formatK\x05gfindK\x06gmatchK\x04gsubK\x03lenK\x05lowerK\x05matchK\x03repK\x07reverseK\x03subK\x05upperR\x00\x00\x01\x00K\x07__indexR\x00\x00\x09\x00K\x06concatK\x07foreachK\x08foreachiK\x04getnK\x04maxnK\x06insertK\x06removeK\x04setnK\x04sort";

pub(crate) fn layout_51(name: &str) -> &'static [u8] {
    match name {
        "FILE*" => &BLOB_51[0..76],
        "_G" => &BLOB_51[76..410],
        "coroutine" => &BLOB_51[410..461],
        "debug" => &BLOB_51[461..614],
        "io" => &BLOB_51[614..719],
        "math" => &BLOB_51[719..909],
        "os" => &BLOB_51[909..1002],
        "package" => &BLOB_51[1002..1071],
        "package.loaders" => &BLOB_51[1071..1076],
        "registry" => &BLOB_51[1076..1107],
        "registry._LOADED" => &BLOB_51[1107..1172],
        "registry._LOADLIB" => &BLOB_51[1172..1178],
        "string" => &BLOB_51[1178..1281],
        "string_mt" => &BLOB_51[1281..1295],
        "table" => &BLOB_51[1295..1367],
        _ => &[],
    }
}

static BLOB_52: &[u8] = b"K\x07__indexK\x05closeK\x05flushK\x05linesK\x04readK\x04seekK\x07setvbufK\x05writeK\x04__gcK\x0a__tostringK\x02_GK\x06assertK\x0ecollectgarbageK\x06dofileK\x05errorK\x0cgetmetatableK\x06ipairsK\x08loadfileK\x04loadK\x0aloadstringK\x04nextK\x05pairsK\x05pcallK\x05printK\x08rawequalK\x06rawlenK\x06rawgetK\x06rawsetK\x06selectK\x0csetmetatableK\x08tonumberK\x08tostringK\x04typeK\x06xpcallK\x08_VERSIONK\x06moduleK\x07requireK\x07packageK\x09coroutineK\x06unpackK\x05tableK\x02ioK\x02osK\x06stringK\x05bit32K\x04mathK\x05debugR\x00\x00\x0c\x00K\x07arshiftK\x04bandK\x04bnotK\x03borK\x04bxorK\x05btestK\x07extractK\x07lrotateK\x06lshiftK\x07replaceK\x07rrotateK\x06rshiftR\x00\x00\x06\x00K\x06createK\x06resumeK\x07runningK\x06statusK\x04wrapK\x05yieldR\x00\x00\x10\x00K\x05debugK\x0cgetuservalueK\x07gethookK\x07getinfoK\x08getlocalK\x0bgetregistryK\x0cgetmetatableK\x0agetupvalueK\x0bupvaluejoinK\x09upvalueidK\x0csetuservalueK\x07sethookK\x08setlocalK\x0csetmetatableK\x0asetupvalueK\x09tracebackR\x00\x00\x0b\x00K\x05closeK\x05flushK\x05inputK\x05linesK\x04openK\x06outputK\x05popenK\x04readK\x07tmpfileK\x04typeK\x05writeK\x05stdinK\x06stdoutK\x06stderrR\x00\x00\x1c\x00K\x03absK\x04acosK\x04asinK\x05atan2K\x04atanK\x04ceilK\x04coshK\x03cosK\x03degK\x03expK\x05floorK\x04fmodK\x05frexpK\x05ldexpK\x05log10K\x03logK\x03maxK\x03minK\x04modfK\x03powK\x03radK\x06randomK\x0arandomseedK\x04sinhK\x03sinK\x04sqrtK\x04tanhK\x03tanK\x02piK\x04hugeR\x00\x00\x0b\x00K\x05clockK\x04dateK\x08difftimeK\x07executeK\x04exitK\x06getenvK\x06removeK\x06renameK\x09setlocaleK\x04timeK\x07tmpnameR\x00\x00\x03\x00K\x07loadlibK\x0asearchpathK\x06seeallK\x07loadersK\x09searchersK\x04pathK\x05cpathK\x06configK\x06loadedK\x07preloadR\x04\x00\x00\x00R\x02\x00\x00\x00K\x07_LOADEDK\x06_CLIBSK\x08_PRELOADK\x05FILE*K\x09_IO_inputK\x0a_IO_outputK\x02_GK\x07packageK\x09coroutineK\x05tableK\x02ioK\x02osK\x06stringK\x05bit32K\x04mathK\x05debugR\x00\x00\x0e\x00K\x04byteK\x04charK\x04dumpK\x04findK\x06formatK\x06gmatchK\x04gsubK\x03lenK\x05lowerK\x05matchK\x03repK\x07reverseK\x03subK\x05upperR\x00\x00\x01\x00K\x07__indexR\x00\x00\x07\x00K\x06concatK\x04maxnK\x06insertK\x04packK\x06unpackK\x06removeK\x04sort";

pub(crate) fn layout_52(name: &str) -> &'static [u8] {
    match name {
        "FILE*" => &BLOB_52[0..76],
        "_G" => &BLOB_52[76..384],
        "bit32" => &BLOB_52[384..480],
        "coroutine" => &BLOB_52[480..531],
        "debug" => &BLOB_52[531..718],
        "io" => &BLOB_52[718..823],
        "math" => &BLOB_52[823..1008],
        "os" => &BLOB_52[1008..1101],
        "package" => &BLOB_52[1101..1193],
        "package.loaders" => &BLOB_52[1193..1198],
        "registry" => &BLOB_52[1198..1260],
        "registry._LOADED" => &BLOB_52[1260..1327],
        "string" => &BLOB_52[1327..1423],
        "string_mt" => &BLOB_52[1423..1437],
        "table" => &BLOB_52[1437..1492],
        _ => &[],
    }
}

static BLOB_53: &[u8] = b"R\x00\x00\x02\x00K\x06__nameK\x07__indexK\x05closeK\x05flushK\x05linesK\x04readK\x04seekK\x07setvbufK\x05writeK\x04__gcK\x0a__tostringK\x06assertK\x0ecollectgarbageK\x06dofileK\x05errorK\x0cgetmetatableK\x06ipairsK\x08loadfileK\x04loadK\x04nextK\x05pairsK\x05pcallK\x05printK\x08rawequalK\x06rawlenK\x06rawgetK\x06rawsetK\x06selectK\x0csetmetatableK\x08tonumberK\x08tostringK\x04typeK\x06xpcallK\x02_GK\x08_VERSIONK\x07requireK\x07packageK\x09coroutineK\x05tableK\x02ioK\x02osK\x06stringK\x04mathK\x04utf8K\x05debugK\x05bit32R\x00\x00\x0c\x00K\x07arshiftK\x04bandK\x04bnotK\x03borK\x04bxorK\x05btestK\x07extractK\x07lrotateK\x06lshiftK\x07replaceK\x07rrotateK\x06rshiftR\x00\x00\x07\x00K\x06createK\x06resumeK\x07runningK\x06statusK\x04wrapK\x05yieldK\x0bisyieldableR\x00\x00\x10\x00K\x05debugK\x0cgetuservalueK\x07gethookK\x07getinfoK\x08getlocalK\x0bgetregistryK\x0cgetmetatableK\x0agetupvalueK\x0bupvaluejoinK\x09upvalueidK\x0csetuservalueK\x07sethookK\x08setlocalK\x0csetmetatableK\x0asetupvalueK\x09tracebackR\x00\x00\x0b\x00K\x05closeK\x05flushK\x05inputK\x05linesK\x04openK\x06outputK\x05popenK\x04readK\x07tmpfileK\x04typeK\x05writeK\x05stdinK\x06stdoutK\x06stderrR\x00\x00#\x00K\x03absK\x04acosK\x04asinK\x04atanK\x04ceilK\x03cosK\x03degK\x03expK\x09tointegerK\x05floorK\x04fmodK\x03ultK\x03logK\x03maxK\x03minK\x04modfK\x03radK\x06randomK\x0arandomseedK\x03sinK\x04sqrtK\x03tanK\x04typeK\x05atan2K\x04coshK\x04sinhK\x04tanhK\x03powK\x05frexpK\x05ldexpK\x05log10K\x02piK\x04hugeK\x0amaxintegerK\x0aminintegerR\x00\x00\x0b\x00K\x05clockK\x04dateK\x08difftimeK\x07executeK\x04exitK\x06getenvK\x06removeK\x06renameK\x09setlocaleK\x04timeK\x07tmpnameR\x00\x00\x07\x00K\x07loadlibK\x0asearchpathK\x07preloadK\x05cpathK\x04pathK\x09searchersK\x06loadedK\x06configR\x04\x00\x00\x00R\x02\x00\x00\x00K\x07_LOADEDK\x08_PRELOADK\x05FILE*K\x09_IO_inputK\x0a_IO_outputK\x02_GK\x07packageK\x09coroutineK\x05tableK\x02ioK\x02osK\x06stringK\x04mathK\x04utf8K\x05debugK\x05bit32R\x00\x00\x11\x00K\x04byteK\x04charK\x04dumpK\x04findK\x06formatK\x06gmatchK\x04gsubK\x03lenK\x05lowerK\x05matchK\x03repK\x07reverseK\x03subK\x05upperK\x04packK\x08packsizeK\x06unpackR\x00\x00\x01\x00K\x07__indexR\x00\x00\x07\x00K\x06concatK\x06insertK\x04packK\x06unpackK\x06removeK\x04moveK\x04sortR\x00\x00\x06\x00K\x06offsetK\x09codepointK\x04charK\x03lenK\x05codesK\x0bcharpattern";

pub(crate) fn layout_53(name: &str) -> &'static [u8] {
    match name {
        "FILE*" => &BLOB_53[0..89],
        "_G" => &BLOB_53[89..375],
        "bit32" => &BLOB_53[375..471],
        "coroutine" => &BLOB_53[471..535],
        "debug" => &BLOB_53[535..722],
        "io" => &BLOB_53[722..827],
        "math" => &BLOB_53[827..1058],
        "os" => &BLOB_53[1058..1151],
        "package" => &BLOB_53[1151..1226],
        "package.searchers" => &BLOB_53[1226..1231],
        "registry" => &BLOB_53[1231..1285],
        "registry._LOADED" => &BLOB_53[1285..1358],
        "string" => &BLOB_53[1358..1478],
        "string_mt" => &BLOB_53[1478..1492],
        "table" => &BLOB_53[1492..1547],
        "utf8" => &BLOB_53[1547..1602],
        _ => &[],
    }
}

static BLOB_54: &[u8] = b"R\x00\x00\x02\x00K\x06__nameK\x07__indexK\x04__gcK\x07__closeK\x0a__tostringR\x00\x00\x07\x00K\x04readK\x05writeK\x05linesK\x05flushK\x04seekK\x05closeK\x07setvbufK\x06assertK\x0ecollectgarbageK\x06dofileK\x05errorK\x0cgetmetatableK\x06ipairsK\x08loadfileK\x04loadK\x04nextK\x05pairsK\x05pcallK\x05printK\x04warnK\x08rawequalK\x06rawlenK\x06rawgetK\x06rawsetK\x06selectK\x0csetmetatableK\x08tonumberK\x08tostringK\x04typeK\x06xpcallK\x02_GK\x08_VERSIONK\x07requireK\x07packageK\x09coroutineK\x05tableK\x02ioK\x02osK\x06stringK\x04mathK\x04utf8K\x05debugR\x00\x00\x08\x00K\x06createK\x06resumeK\x07runningK\x06statusK\x04wrapK\x05yieldK\x0bisyieldableK\x05closeR\x00\x00\x11\x00K\x05debugK\x0cgetuservalueK\x07gethookK\x07getinfoK\x08getlocalK\x0bgetregistryK\x0cgetmetatableK\x0agetupvalueK\x0bupvaluejoinK\x09upvalueidK\x0csetuservalueK\x07sethookK\x08setlocalK\x0csetmetatableK\x0asetupvalueK\x09tracebackK\x0esetcstacklimitR\x00\x00\x0b\x00K\x05closeK\x05flushK\x05inputK\x05linesK\x04openK\x06outputK\x05popenK\x04readK\x07tmpfileK\x04typeK\x05writeK\x05stdinK\x06stdoutK\x06stderrR\x00\x00#\x00K\x03absK\x04acosK\x04asinK\x04atanK\x04ceilK\x03cosK\x03degK\x03expK\x09tointegerK\x05floorK\x04fmodK\x03ultK\x03logK\x03maxK\x03minK\x04modfK\x03radK\x03sinK\x04sqrtK\x03tanK\x04typeK\x05atan2K\x04coshK\x04sinhK\x04tanhK\x03powK\x05frexpK\x05ldexpK\x05log10K\x06randomK\x0arandomseedK\x02piK\x04hugeK\x0amaxintegerK\x0aminintegerR\x00\x00\x0b\x00K\x05clockK\x04dateK\x08difftimeK\x07executeK\x04exitK\x06getenvK\x06removeK\x06renameK\x09setlocaleK\x04timeK\x07tmpnameR\x00\x00\x07\x00K\x07loadlibK\x0asearchpathK\x07preloadK\x05cpathK\x04pathK\x09searchersK\x06loadedK\x06configR\x04\x00\x00\x00R\x02\x00\x00\x00K\x07_LOADEDK\x06_CLIBSK\x08_PRELOADK\x05FILE*K\x09_IO_inputK\x0a_IO_outputK\x02_GK\x07packageK\x09coroutineK\x05tableK\x02ioK\x02osK\x06stringK\x04mathK\x04utf8K\x05debugR\x00\x00\x11\x00K\x04byteK\x04charK\x04dumpK\x04findK\x06formatK\x06gmatchK\x04gsubK\x03lenK\x05lowerK\x05matchK\x03repK\x07reverseK\x03subK\x05upperK\x04packK\x08packsizeK\x06unpackR\x00\x00\x09\x00K\x05__addK\x05__subK\x05__mulK\x05__modK\x05__powK\x05__divK\x06__idivK\x05__unmK\x07__indexR\x00\x00\x07\x00K\x06concatK\x06insertK\x04packK\x06unpackK\x06removeK\x04moveK\x04sortR\x00\x00\x06\x00K\x06offsetK\x09codepointK\x04charK\x03lenK\x05codesK\x0bcharpattern";

pub(crate) fn layout_54(name: &str) -> &'static [u8] {
    match name {
        "FILE*" => &BLOB_54[0..49],
        "FILE*.__index" => &BLOB_54[49..103],
        "_G" => &BLOB_54[103..388],
        "coroutine" => &BLOB_54[388..459],
        "debug" => &BLOB_54[459..662],
        "io" => &BLOB_54[662..767],
        "math" => &BLOB_54[767..998],
        "os" => &BLOB_54[998..1091],
        "package" => &BLOB_54[1091..1166],
        "package.searchers" => &BLOB_54[1166..1171],
        "registry" => &BLOB_54[1171..1233],
        "registry._LOADED" => &BLOB_54[1233..1299],
        "string" => &BLOB_54[1299..1419],
        "string_mt" => &BLOB_54[1419..1490],
        "table" => &BLOB_54[1490..1545],
        "utf8" => &BLOB_54[1545..1600],
        _ => &[],
    }
}

static BLOB_55: &[u8] = b"R\x00\x00\x02\x00K\x06__nameK\x07__indexK\x04__gcK\x07__closeK\x0a__tostringR\x00\x00\x07\x00K\x04readK\x05writeK\x05linesK\x05flushK\x04seekK\x05closeK\x07setvbufK\x06assertK\x0ecollectgarbageK\x06dofileK\x05errorK\x0cgetmetatableK\x06ipairsK\x08loadfileK\x04loadK\x04nextK\x05pairsK\x05pcallK\x05printK\x04warnK\x08rawequalK\x06rawlenK\x06rawgetK\x06rawsetK\x06selectK\x0csetmetatableK\x08tonumberK\x08tostringK\x04typeK\x06xpcallK\x02_GK\x08_VERSIONK\x07requireK\x07packageK\x09coroutineK\x05debugK\x02ioK\x04mathK\x02osK\x06stringK\x05tableK\x04utf8R\x00\x00\x08\x00K\x06createK\x06resumeK\x07runningK\x06statusK\x04wrapK\x05yieldK\x0bisyieldableK\x05closeR\x00\x00\x10\x00K\x05debugK\x0cgetuservalueK\x07gethookK\x07getinfoK\x08getlocalK\x0bgetregistryK\x0cgetmetatableK\x0agetupvalueK\x0bupvaluejoinK\x09upvalueidK\x0csetuservalueK\x07sethookK\x08setlocalK\x0csetmetatableK\x0asetupvalueK\x09tracebackR\x00\x00\x0b\x00K\x05closeK\x05flushK\x05inputK\x05linesK\x04openK\x06outputK\x05popenK\x04readK\x07tmpfileK\x04typeK\x05writeK\x05stdinK\x06stdoutK\x06stderrR\x00\x00\x1d\x00K\x03absK\x04acosK\x04asinK\x04atanK\x04ceilK\x03cosK\x03degK\x03expK\x09tointegerK\x05floorK\x04fmodK\x05frexpK\x03ultK\x05ldexpK\x03logK\x03maxK\x03minK\x04modfK\x03radK\x03sinK\x04sqrtK\x03tanK\x04typeK\x06randomK\x0arandomseedK\x02piK\x04hugeK\x0amaxintegerK\x0aminintegerR\x00\x00\x0b\x00K\x05clockK\x04dateK\x08difftimeK\x07executeK\x04exitK\x06getenvK\x06removeK\x06renameK\x09setlocaleK\x04timeK\x07tmpnameR\x00\x00\x07\x00K\x07loadlibK\x0asearchpathK\x07preloadK\x05cpathK\x04pathK\x09searchersK\x06loadedK\x06configR\x04\x00\x00\x00R\x03\x00\x00\x00K\x08_PRELOADK\x07_LOADEDK\x06_CLIBSK\x05FILE*K\x09_IO_inputK\x0a_IO_outputK\x02_GK\x07packageK\x09coroutineK\x05debugK\x02ioK\x04mathK\x02osK\x06stringK\x05tableK\x04utf8R\x00\x00\x11\x00K\x04byteK\x04charK\x04dumpK\x04findK\x06formatK\x06gmatchK\x04gsubK\x03lenK\x05lowerK\x05matchK\x03repK\x07reverseK\x03subK\x05upperK\x04packK\x08packsizeK\x06unpackR\x00\x00\x09\x00K\x05__addK\x05__subK\x05__mulK\x05__modK\x05__powK\x05__divK\x06__idivK\x05__unmK\x07__indexR\x00\x00\x08\x00K\x06concatK\x06createK\x06insertK\x04packK\x06unpackK\x06removeK\x04moveK\x04sortR\x00\x00\x06\x00K\x06offsetK\x09codepointK\x04charK\x03lenK\x05codesK\x0bcharpattern";

pub(crate) fn layout_55(name: &str) -> &'static [u8] {
    match name {
        "FILE*" => &BLOB_55[0..49],
        "FILE*.__index" => &BLOB_55[49..103],
        "_G" => &BLOB_55[103..388],
        "coroutine" => &BLOB_55[388..459],
        "debug" => &BLOB_55[459..646],
        "io" => &BLOB_55[646..751],
        "math" => &BLOB_55[751..945],
        "os" => &BLOB_55[945..1038],
        "package" => &BLOB_55[1038..1113],
        "package.searchers" => &BLOB_55[1113..1118],
        "registry" => &BLOB_55[1118..1180],
        "registry._LOADED" => &BLOB_55[1180..1246],
        "string" => &BLOB_55[1246..1366],
        "string_mt" => &BLOB_55[1366..1437],
        "table" => &BLOB_55[1437..1500],
        "utf8" => &BLOB_55[1500..1555],
        _ => &[],
    }
}
