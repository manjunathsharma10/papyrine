#include "papyrine_shim.h"
#include "qpdf-sys/src/lib.rs.h"

#include <qpdf/Constants.h>
#include <qpdf/QPDFCryptoImpl.hh>
#include <qpdf/QPDFCryptoProvider.hh>
#include <qpdf/QPDFPageDocumentHelper.hh>
#include <qpdf/QPDFWriter.hh>
#include <qpdf/Types.h>

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <mutex>

static_assert(ot_null == 2 && ot_boolean == 3 && ot_integer == 4 && ot_real == 5 &&
                  ot_string == 6 && ot_name == 7 && ot_array == 8 && ot_dictionary == 9 &&
                  ot_stream == 10,
              "object type numbering is mirrored in papyrine-cos");

namespace papyrine {

namespace {

// Zero-copy InputSource over memory owned by Rust (a Vec, Arc<[u8]> or an mmap).
class SliceInputSource final : public InputSource {
  public:
    SliceInputSource(std::string description, unsigned char const* data, size_t len)
        : description_(std::move(description)), data_(data),
          size_(static_cast<qpdf_offset_t>(len))
    {
    }

    qpdf_offset_t findAndSkipNextEOL() override
    {
        if (pos_ >= size_) {
            last_offset = size_;
            pos_ = size_;
            return size_;
        }
        unsigned char const* base = data_;
        unsigned char const* end = base + size_;
        unsigned char const* p = base + pos_;
        while (p < end && *p != '\r' && *p != '\n') {
            ++p;
        }
        qpdf_offset_t result;
        if (p < end) {
            result = p - base;
            pos_ = result + 1;
            ++p;
            while (pos_ < size_ && (*p == '\r' || *p == '\n')) {
                ++p;
                ++pos_;
            }
        } else {
            pos_ = size_;
            result = size_;
        }
        return result;
    }
    std::string const& getName() const override { return description_; }
    qpdf_offset_t tell() override { return pos_; }
    void seek(qpdf_offset_t offset, int whence) override
    {
        switch (whence) {
        case SEEK_SET:
            pos_ = offset;
            break;
        case SEEK_END:
            pos_ = size_ + offset;
            break;
        default:
            pos_ += offset;
            break;
        }
        if (pos_ < 0) {
            throw std::runtime_error(description_ + ": seek before beginning of buffer");
        }
    }
    void rewind() override { pos_ = 0; }
    size_t read(char* buffer, size_t length) override
    {
        if (pos_ >= size_) {
            last_offset = size_;
            return 0;
        }
        last_offset = pos_;
        size_t len = std::min(static_cast<size_t>(size_ - pos_), length);
        std::memcpy(buffer, data_ + pos_, len);
        pos_ += static_cast<qpdf_offset_t>(len);
        return len;
    }
    void unreadCh(char) override
    {
        if (pos_ > 0) {
            --pos_;
        }
    }

  private:
    std::string description_;
    unsigned char const* data_;
    qpdf_offset_t size_;
    qpdf_offset_t pos_ = 0;
};

std::string to_string(rust::Slice<uint8_t const> s)
{
    return std::string(reinterpret_cast<char const*>(s.data()), s.size());
}

std::string to_string(rust::Vec<uint8_t> const& s)
{
    return std::string(reinterpret_cast<char const*>(s.data()), s.size());
}

rust::Vec<uint8_t> to_vec(std::string const& s)
{
    rust::Vec<uint8_t> v;
    v.reserve(s.size());
    for (unsigned char c : s) {
        v.push_back(c);
    }
    return v;
}

std::unique_ptr<Obj> wrap(QPDFObjectHandle h)
{
    return std::make_unique<Obj>(Obj{std::move(h)});
}

[[noreturn]] void type_error(char const* expected)
{
    throw ShimError(kTypeError, std::string("type error: expected ") + expected);
}

void configure_open(Doc const& d, OpenOptions const& opts)
{
    d.q->setSuppressWarnings(true);
    d.q->setAttemptRecovery(opts.attempt_recovery);
    d.q->setIgnoreXRefStreams(opts.ignore_xref_streams);
    if (opts.max_warnings > 0) {
        d.q->setMaxWarnings(opts.max_warnings);
    }
}

int method_code(QPDF::encryption_method_e m)
{
    switch (m) {
    case QPDF::e_none:
        return 0;
    case QPDF::e_unknown:
        return 1;
    case QPDF::e_rc4:
        return 2;
    case QPDF::e_aes:
        return 3;
    case QPDF::e_aesv3:
        return 4;
    }
    return 1;
}

qpdf_r3_print_e print_mode(int p)
{
    return p >= 2 ? qpdf_r3p_full : (p == 1 ? qpdf_r3p_low : qpdf_r3p_none);
}

void check_dict(Obj const& o)
{
    if (!o.h.isDictionary() && !o.h.isStream()) {
        type_error("dictionary or stream");
    }
}

void check_array(Obj const& o)
{
    if (!o.h.isArray()) {
        type_error("array");
    }
}

std::string slash_key(rust::Slice<uint8_t const> key)
{
    return to_string(key);
}

} // namespace

std::unique_ptr<Doc> doc_new()
{
    // Handles that are not attached to a document warn through qpdf's process-wide default
    // logger (stderr). Silence it once; per-document warnings are captured structurally.
    static std::once_flag silenced;
    std::call_once(silenced, [] {
        auto def = QPDFLogger::defaultLogger();
        auto discard = def->discard();
        def->setInfo(discard);
        def->setWarn(discard);
        def->setError(discard);
    });
    auto d = std::make_unique<Doc>();
    d->q = QPDF::create();
    d->logger = QPDFLogger::create();
    auto discard = d->logger->discard();
    d->logger->setInfo(discard);
    d->logger->setWarn(discard);
    d->logger->setError(discard);
    d->q->setLogger(d->logger);
    d->q->setSuppressWarnings(true);
    return d;
}

void doc_open_slice(Doc const& d, rust::Str description, uint8_t const* data, size_t len,
                    OpenOptions const& opts)
{
    configure_open(d, opts);
    d.src = std::make_shared<SliceInputSource>(std::string(description), data, len);
    auto pw = to_string(opts.password);
    d.q->processInputSource(d.src, pw.c_str());
}

void doc_open_file(Doc const& d, rust::Slice<uint8_t const> path, OpenOptions const& opts)
{
    configure_open(d, opts);
    auto p = to_string(path);
    auto pw = to_string(opts.password);
    d.q->processFile(p.c_str(), pw.c_str());
}

void doc_new_empty(Doc const& d)
{
    d.q->emptyPDF();
}

rust::Vec<RepairEntry> doc_take_warnings(Doc const& d)
{
    rust::Vec<RepairEntry> out;
    for (auto const& w : d.q->getWarnings()) {
        RepairEntry e;
        e.message = rust::String::lossy(w.getMessageDetail());
        e.filename = rust::String::lossy(w.getFilename());
        e.object = rust::String::lossy(w.getObject());
        int id = 0;
        int gen = 0;
        if (std::sscanf(w.getObject().c_str(), "object %d %d", &id, &gen) != 2) {
            id = 0;
            gen = 0;
        }
        e.object_id = id;
        e.object_gen = gen;
        e.offset = static_cast<int64_t>(w.getFilePosition());
        e.code = static_cast<int32_t>(w.getErrorCode());
        out.push_back(std::move(e));
    }
    return out;
}

rust::String doc_version(Doc const& d)
{
    return rust::String::lossy(d.q->getPDFVersion());
}

rust::String qpdf_version()
{
    return rust::String::lossy(QPDF::QPDFVersion());
}

bool doc_is_linearized(Doc const& d)
{
    return d.q->isLinearized();
}

EncryptionInfo doc_encryption_info(Doc const& d)
{
    EncryptionInfo i{};
    int r = 0, p = 0, v = 0;
    QPDF::encryption_method_e sm = QPDF::e_none, strm = QPDF::e_none, fm = QPDF::e_none;
    i.encrypted = d.q->isEncrypted(r, p, v, sm, strm, fm);
    if (i.encrypted) {
        i.r = r;
        i.p = p;
        i.v = v;
        i.stream_method = method_code(sm);
        i.string_method = method_code(strm);
        i.file_method = method_code(fm);
        i.user_password_matched = d.q->userPasswordMatched();
        i.owner_password_matched = d.q->ownerPasswordMatched();
    }
    i.allow_accessibility = d.q->allowAccessibility();
    i.allow_extract_all = d.q->allowExtractAll();
    i.allow_print_low_res = d.q->allowPrintLowRes();
    i.allow_print_high_res = d.q->allowPrintHighRes();
    i.allow_modify_assembly = d.q->allowModifyAssembly();
    i.allow_modify_form = d.q->allowModifyForm();
    i.allow_modify_annotation = d.q->allowModifyAnnotation();
    i.allow_modify_other = d.q->allowModifyOther();
    i.allow_modify_all = d.q->allowModifyAll();
    return i;
}

std::unique_ptr<Buf> doc_encryption_key(Doc const& d)
{
    auto b = std::make_unique<Buf>();
    b->str = d.q->getEncryptionKey();
    return b;
}

std::unique_ptr<Obj> doc_trailer(Doc const& d)
{
    return wrap(d.q->getTrailer());
}

std::unique_ptr<Obj> doc_root(Doc const& d)
{
    return wrap(d.q->getRoot());
}

size_t doc_object_count(Doc const& d)
{
    return d.q->getObjectCount();
}

rust::Vec<ObjGenPair> doc_all_objects(Doc const& d)
{
    rust::Vec<ObjGenPair> out;
    for (auto const& o : d.q->getAllObjects()) {
        auto og = o.getObjGen();
        out.push_back(ObjGenPair{og.getObj(), og.getGen()});
    }
    return out;
}

std::unique_ptr<Obj> doc_get_object(Doc const& d, int32_t id, int32_t gen_)
{
    return wrap(d.q->getObject(id, gen_));
}

std::unique_ptr<Obj> doc_make_indirect(Doc const& d, Obj const& o)
{
    return wrap(d.q->makeIndirectObject(o.h));
}

void doc_replace_object(Doc const& d, int32_t id, int32_t gen_, Obj const& o)
{
    d.q->replaceObject(id, gen_, o.h);
}

std::unique_ptr<Obj> doc_copy_foreign(Doc const& d, Obj const& foreign)
{
    return wrap(d.q->copyForeignObject(foreign.h));
}

std::unique_ptr<Obj> doc_new_stream(Doc const& d, rust::Slice<uint8_t const> data)
{
    return wrap(d.q->newStream(to_string(data)));
}

size_t doc_pages_count(Doc const& d)
{
    return d.q->getAllPages().size();
}

std::unique_ptr<Obj> doc_page(Doc const& d, size_t index)
{
    auto const& pages = d.q->getAllPages();
    if (index >= pages.size()) {
        throw ShimError(kRangeError, "page index out of range");
    }
    return wrap(pages[index]);
}

void doc_add_page(Doc const& d, Obj const& page, bool first)
{
    d.q->addPage(page.h, first);
}

void doc_add_page_at(Doc const& d, Obj const& page, bool before, Obj const& reference)
{
    d.q->addPageAt(page.h, before, reference.h);
}

void doc_remove_page(Doc const& d, Obj const& page)
{
    d.q->removePage(page.h);
}

int32_t doc_find_page(Doc const& d, Obj const& page)
{
    QPDFObjectHandle h = page.h;
    return d.q->findPage(h);
}

void doc_push_inherited(Doc const& d)
{
    d.q->pushInheritedAttributesToPage();
}

std::unique_ptr<WriteOut> doc_write(Doc const& d, WriteOptions const& opts,
                                    rust::Slice<uint8_t const> path)
{
    auto out = std::make_unique<WriteOut>();
    std::vector<QPDFObjGen> ogs;
    for (auto const& o : d.q->getAllObjects()) {
        ogs.push_back(o.getObjGen());
    }

    QPDFWriter w(*d.q);
    std::string path_s = to_string(path);
    bool to_memory = path_s.empty();
    if (to_memory) {
        w.setOutputMemory();
    } else {
        w.setOutputFilename(path_s.c_str());
    }
    w.setObjectStreamMode(opts.object_streams == 0   ? qpdf_o_disable
                          : opts.object_streams == 1 ? qpdf_o_preserve
                                                     : qpdf_o_generate);
    w.setStreamDataMode(opts.stream_data == 0   ? qpdf_s_uncompress
                        : opts.stream_data == 1 ? qpdf_s_preserve
                                                : qpdf_s_compress);
    w.setRecompressFlate(opts.recompress_flate);
    w.setQDFMode(opts.qdf);
    w.setContentNormalization(opts.normalize_content);
    w.setPreserveUnreferencedObjects(opts.preserve_unreferenced);
    w.setNewlineBeforeEndstream(opts.newline_before_endstream);
    w.setStaticID(opts.static_id);
    w.setDeterministicID(opts.deterministic_id);
    if (!opts.min_version.empty()) {
        w.setMinimumPDFVersion(std::string(opts.min_version));
    }
    w.setPreserveEncryption(opts.preserve_encryption);

    auto const& e = opts.encryption;
    if (e.r != 0) {
        auto up = to_string(e.user_password);
        auto op = to_string(e.owner_password);
        switch (e.r) {
        case 2:
            w.setR2EncryptionParametersInsecure(up.c_str(), op.c_str(), e.print > 0,
                                                e.allow_modify_other, e.allow_extract,
                                                e.allow_annotate_and_form);
            break;
        case 3:
            w.setR3EncryptionParametersInsecure(up.c_str(), op.c_str(), e.allow_accessibility,
                                                e.allow_extract, e.allow_assemble,
                                                e.allow_annotate_and_form, e.allow_form_filling,
                                                e.allow_modify_other, print_mode(e.print));
            break;
        case 4:
            w.setR4EncryptionParametersInsecure(
                up.c_str(), op.c_str(), e.allow_accessibility, e.allow_extract, e.allow_assemble,
                e.allow_annotate_and_form, e.allow_form_filling, e.allow_modify_other,
                print_mode(e.print), e.encrypt_metadata, e.aes);
            break;
        case 5:
            w.setR5EncryptionParameters(up.c_str(), op.c_str(), e.allow_accessibility,
                                        e.allow_extract, e.allow_assemble,
                                        e.allow_annotate_and_form, e.allow_form_filling,
                                        e.allow_modify_other, print_mode(e.print),
                                        e.encrypt_metadata);
            break;
        case 6:
            w.setR6EncryptionParameters(up.c_str(), op.c_str(), e.allow_accessibility,
                                        e.allow_extract, e.allow_assemble,
                                        e.allow_annotate_and_form, e.allow_form_filling,
                                        e.allow_modify_other, print_mode(e.print),
                                        e.encrypt_metadata);
            break;
        default:
            throw ShimError(kRangeError, "unsupported encryption revision");
        }
    }
    // Linearization disables QDF/normalisation internally; set it last.
    w.setLinearization(opts.linearize);

    w.write();

    if (to_memory) {
        out->buf = w.getBufferSharedPointer();
    }
    for (auto const& og : ogs) {
        QPDFObjGen n = w.getRenumberedObjGen(og);
        if (n.isIndirect()) {
            out->renumber.insert(out->renumber.end(),
                                 {og.getObj(), og.getGen(), n.getObj(), n.getGen()});
        }
    }
    return out;
}

rust::Slice<uint8_t const> write_out_data(WriteOut const& w)
{
    if (!w.buf) {
        return {};
    }
    return rust::Slice<uint8_t const>(w.buf->getBuffer(), w.buf->getSize());
}

rust::Vec<Renumber> write_out_renumber(WriteOut const& w)
{
    rust::Vec<Renumber> out;
    for (size_t i = 0; i + 3 < w.renumber.size(); i += 4) {
        out.push_back(Renumber{w.renumber[i], w.renumber[i + 1], w.renumber[i + 2],
                               w.renumber[i + 3]});
    }
    return out;
}

rust::Vec<rust::String> crypto_impls()
{
    rust::Vec<rust::String> out;
    for (auto const& n : QPDFCryptoProvider::getRegisteredImpls()) {
        out.push_back(rust::String::lossy(n));
    }
    return out;
}

rust::String crypto_default()
{
    return rust::String::lossy(QPDFCryptoProvider::getDefaultProvider());
}

std::unique_ptr<Obj> obj_clone(Obj const& o)
{
    return wrap(o.h);
}

int32_t obj_type(Obj const& o)
{
    return static_cast<int32_t>(o.h.getTypeCode());
}

bool obj_is_indirect(Obj const& o)
{
    return o.h.isIndirect();
}

int32_t obj_id(Obj const& o)
{
    return o.h.getObjectID();
}

int32_t obj_gen(Obj const& o)
{
    return o.h.getGeneration();
}

bool obj_get_bool(Obj const& o)
{
    if (!o.h.isBool()) {
        type_error("boolean");
    }
    return o.h.getBoolValue();
}

int64_t obj_get_int(Obj const& o)
{
    if (!o.h.isInteger()) {
        type_error("integer");
    }
    return o.h.getIntValue();
}

double obj_get_numeric(Obj const& o)
{
    if (!o.h.isNumber()) {
        type_error("number");
    }
    return o.h.getNumericValue();
}

rust::String obj_get_real_text(Obj const& o)
{
    if (!o.h.isReal()) {
        type_error("real");
    }
    return rust::String::lossy(o.h.getRealValue());
}

rust::Vec<uint8_t> obj_get_name(Obj const& o)
{
    if (!o.h.isName()) {
        type_error("name");
    }
    return to_vec(o.h.getName());
}

rust::Vec<uint8_t> obj_get_string(Obj const& o)
{
    if (!o.h.isString()) {
        type_error("string");
    }
    return to_vec(o.h.getStringValue());
}

rust::Vec<uint8_t> obj_unparse(Obj const& o, bool resolved)
{
    return to_vec(resolved ? o.h.unparseResolved() : o.h.unparse());
}

Fingerprint obj_fingerprint(Obj const& o)
{
    Fingerprint f{};
    QPDFObjectHandle h = o.h;
    f.is_stream = h.isStream();
    QPDFObjectHandle direct = f.is_stream ? h.getDict() : h;
    if (direct.isDictionary() || direct.isArray()) {
        direct = direct.shallowCopy();
        f.repr = to_vec(direct.unparseBinary());
    } else {
        f.repr = to_vec(direct.unparseResolved());
    }
    if (f.is_stream) {
        auto raw = h.getRawStreamData();
        f.stream_raw_len = raw->getSize();
        auto impl = QPDFCryptoProvider::getImpl();
        impl->SHA2_init(256);
        impl->SHA2_update(raw->getBuffer(), raw->getSize());
        impl->SHA2_finalize();
        f.stream_sha256 = to_vec(impl->SHA2_digest());
    }
    return f;
}

std::unique_ptr<Obj> obj_new_null()
{
    return wrap(QPDFObjectHandle::newNull());
}
std::unique_ptr<Obj> obj_new_bool(bool v)
{
    return wrap(QPDFObjectHandle::newBool(v));
}
std::unique_ptr<Obj> obj_new_int(int64_t v)
{
    return wrap(QPDFObjectHandle::newInteger(v));
}
std::unique_ptr<Obj> obj_new_real(double v)
{
    return wrap(QPDFObjectHandle::newReal(v, 6, true));
}
std::unique_ptr<Obj> obj_new_name(rust::Slice<uint8_t const> v)
{
    auto s = to_string(v);
    if (s.empty() || s[0] != '/') {
        throw ShimError(kRangeError, "name must start with '/'");
    }
    return wrap(QPDFObjectHandle::newName(s));
}
std::unique_ptr<Obj> obj_new_string(rust::Slice<uint8_t const> v)
{
    return wrap(QPDFObjectHandle::newString(to_string(v)));
}
std::unique_ptr<Obj> obj_new_array()
{
    return wrap(QPDFObjectHandle::newArray());
}
std::unique_ptr<Obj> obj_new_dict()
{
    return wrap(QPDFObjectHandle::newDictionary());
}
std::unique_ptr<Obj> obj_parse(rust::Slice<uint8_t const> text)
{
    return wrap(QPDFObjectHandle::parse(to_string(text)));
}

int32_t array_len(Obj const& o)
{
    check_array(o);
    return o.h.getArrayNItems();
}

std::unique_ptr<Obj> array_get(Obj const& o, int32_t i)
{
    check_array(o);
    if (i < 0 || i >= o.h.getArrayNItems()) {
        throw ShimError(kRangeError, "array index out of range");
    }
    return wrap(o.h.getArrayItem(i));
}

void array_set(Obj const& o, int32_t i, Obj const& v)
{
    check_array(o);
    if (i < 0 || i >= o.h.getArrayNItems()) {
        throw ShimError(kRangeError, "array index out of range");
    }
    QPDFObjectHandle h = o.h;
    h.setArrayItem(i, v.h);
}

void array_append(Obj const& o, Obj const& v)
{
    check_array(o);
    QPDFObjectHandle h = o.h;
    h.appendItem(v.h);
}

void array_insert(Obj const& o, int32_t i, Obj const& v)
{
    check_array(o);
    if (i < 0 || i > o.h.getArrayNItems()) {
        throw ShimError(kRangeError, "array index out of range");
    }
    QPDFObjectHandle h = o.h;
    h.insertItem(i, v.h);
}

void array_erase(Obj const& o, int32_t i)
{
    check_array(o);
    if (i < 0 || i >= o.h.getArrayNItems()) {
        throw ShimError(kRangeError, "array index out of range");
    }
    QPDFObjectHandle h = o.h;
    h.eraseItem(i);
}

rust::Vec<Bytes> dict_keys(Obj const& o)
{
    check_dict(o);
    rust::Vec<Bytes> out;
    for (auto const& k : o.h.getKeys()) {
        out.push_back(Bytes{to_vec(k)});
    }
    return out;
}

bool dict_has(Obj const& o, rust::Slice<uint8_t const> key)
{
    check_dict(o);
    return o.h.hasKey(slash_key(key));
}

std::unique_ptr<Obj> dict_get(Obj const& o, rust::Slice<uint8_t const> key)
{
    check_dict(o);
    return wrap(o.h.getKey(slash_key(key)));
}

void dict_replace(Obj const& o, rust::Slice<uint8_t const> key, Obj const& v)
{
    check_dict(o);
    QPDFObjectHandle h = o.h;
    h.replaceKey(slash_key(key), v.h);
}

void dict_remove(Obj const& o, rust::Slice<uint8_t const> key)
{
    check_dict(o);
    QPDFObjectHandle h = o.h;
    h.removeKey(slash_key(key));
}

std::unique_ptr<Obj> stream_dict(Obj const& o)
{
    if (!o.h.isStream()) {
        type_error("stream");
    }
    return wrap(o.h.getDict());
}

std::unique_ptr<Buf> stream_raw_data(Obj const& o)
{
    if (!o.h.isStream()) {
        type_error("stream");
    }
    auto b = std::make_unique<Buf>();
    QPDFObjectHandle h = o.h;
    b->buf = h.getRawStreamData();
    return b;
}

std::unique_ptr<Buf> stream_data(Obj const& o, int32_t decode_level)
{
    if (!o.h.isStream()) {
        type_error("stream");
    }
    auto b = std::make_unique<Buf>();
    QPDFObjectHandle h = o.h;
    auto lvl = static_cast<qpdf_stream_decode_level_e>(std::clamp(decode_level, 0, 3));
    b->buf = h.getStreamData(lvl);
    return b;
}

void stream_replace_data(Obj const& o, rust::Slice<uint8_t const> data, Obj const& filter,
                         Obj const& decode_parms)
{
    if (!o.h.isStream()) {
        type_error("stream");
    }
    QPDFObjectHandle h = o.h;
    h.replaceStreamData(to_string(data), filter.h, decode_parms.h);
}

rust::Slice<uint8_t const> buf_data(Buf const& b)
{
    if (b.buf) {
        return rust::Slice<uint8_t const>(b.buf->getBuffer(), b.buf->getSize());
    }
    return rust::Slice<uint8_t const>(reinterpret_cast<uint8_t const*>(b.str.data()),
                                      b.str.size());
}

} // namespace papyrine
