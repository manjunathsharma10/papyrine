// C++ shim over the qpdf C++ API, bridged with cxx (see src/lib.rs).
// Every function reachable from Rust is wrapped by the custom `trycatch` below, so no C++
// exception can unwind into Rust: errors become `Result::Err` carrying "<code>|<message>".
#pragma once

#include <cstdint>
#include <exception>
#include <memory>
#include <new>
#include <stdexcept>
#include <string>
#include <vector>

#include "rust/cxx.h"

#include <qpdf/Buffer.hh>
#include <qpdf/InputSource.hh>
#include <qpdf/QPDF.hh>
#include <qpdf/QPDFExc.hh>
#include <qpdf/QPDFLogger.hh>
#include <qpdf/QPDFSystemError.hh>
#include <qpdf/QPDFObjectHandle.hh>

namespace papyrine {
// Precondition failures raised by the shim itself (wrong object type, index out of range, ...).
struct ShimError : std::runtime_error {
    int code;
    ShimError(int c, std::string const& m) : std::runtime_error(m), code(c) {}
};
inline constexpr int kTypeError = 110;
inline constexpr int kRangeError = 111;
inline constexpr int kCancelled = 112;
} // namespace papyrine

namespace rust::behavior {
template <typename Try, typename Fail>
static void trycatch(Try&& func, Fail&& fail) noexcept
{
    try {
        func();
    } catch (QPDFExc const& e) {
        fail((std::to_string(static_cast<int>(e.getErrorCode())) + "|" + e.what()).c_str());
    } catch (QPDFSystemError const& e) {
        fail((std::string("2|") + e.what()).c_str());
    } catch (papyrine::ShimError const& e) {
        fail((std::to_string(e.code) + "|" + e.what()).c_str());
    } catch (std::range_error const& e) {
        // QIntC conversions on out-of-range numbers read from a damaged file.
        fail((std::string("5|") + e.what()).c_str());
    } catch (std::out_of_range const& e) {
        fail((std::string("5|") + e.what()).c_str());
    } catch (std::length_error const& e) {
        fail((std::string("5|") + e.what()).c_str());
    } catch (std::bad_alloc const&) {
        fail("102|out of memory");
    } catch (std::exception const& e) {
        fail((std::string("100|") + e.what()).c_str());
    } catch (...) {
        fail("101|unknown C++ exception");
    }
}
} // namespace rust::behavior

namespace papyrine {

struct Bytes;
struct ObjGenPair;
struct RepairEntry;
struct OpenOptions;
struct EncryptionInfo;
struct EncryptionParams;
struct WriteOptions;
struct Renumber;
struct Fingerprint;
struct WidgetInfo;
struct FieldInfo;
struct OutlineItem;
struct LabelRange;
struct EmbeddedFileInfo;
struct ProviderBox;
struct ProgressBox;

// A document: the QPDF instance plus everything that must outlive it.
struct Doc {
    mutable std::shared_ptr<QPDF> q;
    mutable std::shared_ptr<InputSource> src;
    mutable std::shared_ptr<QPDFLogger> logger;
};

struct Obj {
    QPDFObjectHandle h;
};

struct WriteOut {
    std::shared_ptr<Buffer> buf;
    std::vector<int32_t> renumber; // old_id, old_gen, new_id, new_gen, ...
};

// Owned byte buffer handed to Rust as a borrowed slice (no copy across the boundary).
struct Buf {
    std::shared_ptr<Buffer> buf;
    std::string str;
};

std::unique_ptr<Doc> doc_new();
void doc_open_slice(Doc const& d, rust::Str description, uint8_t const* data, size_t len,
                    OpenOptions const& opts);
void doc_open_file(Doc const& d, rust::Slice<uint8_t const> path, OpenOptions const& opts);
void doc_new_empty(Doc const& d);
rust::Vec<RepairEntry> doc_take_warnings(Doc const& d);
rust::String doc_version(Doc const& d);
rust::String qpdf_version();
bool doc_is_linearized(Doc const& d);
EncryptionInfo doc_encryption_info(Doc const& d);
std::unique_ptr<Buf> doc_encryption_key(Doc const& d);
std::unique_ptr<Obj> doc_trailer(Doc const& d);
std::unique_ptr<Obj> doc_root(Doc const& d);
size_t doc_object_count(Doc const& d);
rust::Vec<ObjGenPair> doc_all_objects(Doc const& d);
std::unique_ptr<Obj> doc_get_object(Doc const& d, int32_t id, int32_t gen);
std::unique_ptr<Obj> doc_make_indirect(Doc const& d, Obj const& o);
void doc_replace_object(Doc const& d, int32_t id, int32_t gen, Obj const& o);
std::unique_ptr<Obj> doc_copy_foreign(Doc const& d, Obj const& foreign);
std::unique_ptr<Obj> doc_new_stream(Doc const& d, rust::Slice<uint8_t const> data);
size_t doc_pages_count(Doc const& d);
std::unique_ptr<Obj> doc_page(Doc const& d, size_t index);
void doc_add_page(Doc const& d, Obj const& page, bool first);
void doc_add_page_at(Doc const& d, Obj const& page, bool before, Obj const& reference);
void doc_remove_page(Doc const& d, Obj const& page);
int32_t doc_find_page(Doc const& d, Obj const& page);
void doc_push_inherited(Doc const& d);
std::unique_ptr<WriteOut> doc_write(Doc const& d, WriteOptions const& opts,
                                    rust::Slice<uint8_t const> path, ProgressBox& progress);
rust::Slice<uint8_t const> write_out_data(WriteOut const& w);
rust::Vec<Renumber> write_out_renumber(WriteOut const& w);

rust::Vec<rust::String> crypto_impls();
rust::String crypto_default();

std::unique_ptr<Obj> obj_clone(Obj const& o);
int32_t obj_type(Obj const& o);
bool obj_is_indirect(Obj const& o);
int32_t obj_id(Obj const& o);
int32_t obj_gen(Obj const& o);
bool obj_get_bool(Obj const& o);
int64_t obj_get_int(Obj const& o);
double obj_get_numeric(Obj const& o);
rust::String obj_get_real_text(Obj const& o);
rust::Vec<uint8_t> obj_get_name(Obj const& o);
rust::Vec<uint8_t> obj_get_string(Obj const& o);
rust::Vec<uint8_t> obj_unparse(Obj const& o, bool resolved);
Fingerprint obj_fingerprint(Obj const& o);

std::unique_ptr<Obj> obj_new_null();
std::unique_ptr<Obj> obj_new_bool(bool v);
std::unique_ptr<Obj> obj_new_int(int64_t v);
std::unique_ptr<Obj> obj_new_real(double v);
std::unique_ptr<Obj> obj_new_name(rust::Slice<uint8_t const> v);
std::unique_ptr<Obj> obj_new_string(rust::Slice<uint8_t const> v);
std::unique_ptr<Obj> obj_new_array();
std::unique_ptr<Obj> obj_new_dict();
std::unique_ptr<Obj> obj_parse(rust::Slice<uint8_t const> text);

int32_t array_len(Obj const& o);
std::unique_ptr<Obj> array_get(Obj const& o, int32_t i);
void array_set(Obj const& o, int32_t i, Obj const& v);
void array_append(Obj const& o, Obj const& v);
void array_insert(Obj const& o, int32_t i, Obj const& v);
void array_erase(Obj const& o, int32_t i);

rust::Vec<Bytes> dict_keys(Obj const& o);
bool dict_has(Obj const& o, rust::Slice<uint8_t const> key);
std::unique_ptr<Obj> dict_get(Obj const& o, rust::Slice<uint8_t const> key);
void dict_replace(Obj const& o, rust::Slice<uint8_t const> key, Obj const& v);
void dict_remove(Obj const& o, rust::Slice<uint8_t const> key);

std::unique_ptr<Obj> stream_dict(Obj const& o);
std::unique_ptr<Buf> stream_raw_data(Obj const& o);
std::unique_ptr<Buf> stream_data(Obj const& o, int32_t decode_level);
void stream_replace_data(Obj const& o, rust::Slice<uint8_t const> data, Obj const& filter,
                         Obj const& decode_parms);

void stream_replace_provider(Obj const& o, rust::Box<ProviderBox> provider, Obj const& filter,
                             Obj const& decode_parms);

rust::Slice<uint8_t const> buf_data(Buf const& b);

bool form_has_acroform(Doc const& d);
rust::Vec<FieldInfo> form_fields(Doc const& d);
void form_set_value(Doc const& d, int32_t id, int32_t gen, rust::Slice<uint8_t const> value,
                    bool appearance);
bool form_need_appearances(Doc const& d);
void form_set_need_appearances(Doc const& d, bool v);
void form_generate_appearances(Doc const& d);

void flatten_annotations(Doc const& d, int32_t required_flags, int32_t forbidden_flags);
void remove_unreferenced_resources(Doc const& d);
void page_remove_unreferenced_resources(Doc const& d, Obj const& page);
rust::Vec<ObjGenPair> copy_pages(Doc const& d, Doc const& src, rust::Slice<uint32_t const> indices,
                                 size_t at);

rust::Vec<OutlineItem> outlines_read(Doc const& d);
rust::Vec<LabelRange> page_labels_read(Doc const& d);
void page_labels_write(Doc const& d, rust::Slice<LabelRange const> ranges);
rust::Vec<EmbeddedFileInfo> embedded_files_list(Doc const& d);

std::unique_ptr<Obj> name_tree_new(Doc const& d);
rust::Vec<Bytes> name_tree_keys(Doc const& d, Obj const& root);
std::unique_ptr<Obj> name_tree_get(Doc const& d, Obj const& root, rust::Slice<uint8_t const> key);
void name_tree_set(Doc const& d, Obj const& root, rust::Slice<uint8_t const> key, Obj const& value);
bool name_tree_remove(Doc const& d, Obj const& root, rust::Slice<uint8_t const> key);
std::unique_ptr<Obj> number_tree_new(Doc const& d);
rust::Vec<int64_t> number_tree_keys(Doc const& d, Obj const& root);
std::unique_ptr<Obj> number_tree_get(Doc const& d, Obj const& root, int64_t key);
void number_tree_set(Doc const& d, Obj const& root, int64_t key, Obj const& value);
bool number_tree_remove(Doc const& d, Obj const& root, int64_t key);

} // namespace papyrine
