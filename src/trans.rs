use std::any::Any;
use std::boxed::BoxAny;
use std::collections::{HashMap, HashSet};

use rustc::metadata::csearch;
use rustc::middle::astencode;
use rustc::middle::def;
use rustc::middle::region;
use rustc::middle::subst::ParamSpace::*;
use rustc::middle::subst::ParamSpace;
use rustc::middle::subst;
use rustc::middle::ty;
use rustc::middle::ty::{MethodCall, MethodCallee, MethodOrigin};
//use rustc::middle::typeck;
use rustc::util::ppaux::Repr;
use syntax::ast::*;
use syntax::ast_map;
use syntax::ast_util::local_def;
use syntax::codemap::Span;
use syntax::ptr::P;
use syntax::visit::Visitor;
use syntax::visit::{FnKind, FkItemFn, FkMethod, FkFnBlock};
use syntax::visit;

struct TransCtxt<'a, 'tcx: 'a> {
    tcx: &'a ty::ctxt<'tcx>,
    observed_abstract_fns: HashMap<String, DefId>,
    observed_abstract_types: HashMap<String, DefId>,
    crate_name: String,
}

trait Trans {
    fn trans(&self, trcx: &mut TransCtxt) -> String;
}

trait TransExtra<E> {
    fn trans_extra(&self, trcx: &mut TransCtxt, extra: E) -> String;
}

/*
impl<T: Trans, E> TransExtra<E> for T {
    fn trans_extra(&self, trcx: &mut TransCtxt, _: E) -> String {
        self.trans(trcx)
    }
}
*/

/*
impl<T: TransExtra<()>> Trans for T {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        self.trans_extra(trcx, ())
    }
}
*/

impl<T: Trans> Trans for Option<T> {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        self.as_slice().trans(trcx)
    }
}

impl<T: Trans> Trans for Vec<T> {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        self.as_slice().trans(trcx)
    }
}

impl<T: TransExtra<E>, E: Copy> TransExtra<E> for Vec<T> {
    fn trans_extra(&self, trcx: &mut TransCtxt, extra: E) -> String {
        self.as_slice().trans_extra(trcx, extra)
    }
}

impl<'a, T: Trans> Trans for &'a [T] {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        let mut result = format!("{}", self.len());
        for item in self.iter() {
            result.push_str(" ");
            result.push_str(item.trans(trcx).as_slice());
        }
        result
    }
}

impl<'a, T: TransExtra<E>, E: Copy> TransExtra<E> for &'a [T] {
    fn trans_extra(&self, trcx: &mut TransCtxt, extra: E) -> String {
        let mut result = format!("{}", self.len());
        for item in self.iter() {
            result.push_str(" ");
            result.push_str(item.trans_extra(trcx, extra).as_slice());
        }
        result
    }
}

impl<T: Trans> Trans for P<T> {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        (**self).trans(trcx)
    }
}

impl<T: TransExtra<E>, E> TransExtra<E> for P<T> {
    fn trans_extra(&self, trcx: &mut TransCtxt, extra: E) -> String {
        (**self).trans_extra(trcx, extra)
    }
}

impl Trans for String {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        self.clone()
    }
}

impl Trans for Ident {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        format!("{}", self.as_str())
    }
}

impl Trans for Name {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        format!("{}", self.as_str())
    }
}

impl Trans for ParamSpace {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        format!("{}",
                match *self {
                    TypeSpace => "t_",
                    SelfSpace => "s_",
                    FnSpace => "f_",
                })
    }
}

impl TransExtra<ParamSpace> for Generics {
    fn trans_extra(&self, trcx: &mut TransCtxt, space: ParamSpace) -> String {
        let mut lifetimes = vec![];
        for i in range(0, self.lifetimes.len()) {
            //lifetimes.push(format!("{}{}", space.trans(trcx), i));
            lifetimes.push(format!("r_named_0_{}", self.lifetimes[i].lifetime.id));
        }

        let mut ty_params = vec![];
        for i in range(0, self.ty_params.len()) {
            ty_params.push(format!("{}{}", space.trans(trcx), i));
        }

        format!("{} {}",
                lifetimes.trans(trcx),
                ty_params.trans(trcx))
    }
}

/*
impl TransExtra<ParamSpace> for LifetimeDef {
    fn trans_extra(&self, trcx: &mut TransCtxt, space: ParamSpace) -> String {
        format!("{}{}",
                space.trans(trcx),
                self.lifetime.name.trans(trcx))
    }
}

impl TransExtra<ParamSpace> for TyParam {
    fn trans_extra(&self, trcx: &mut TransCtxt, space: ParamSpace) -> String {
        format!("{}{}",
                space.trans(trcx),
                self.ident.trans(trcx))
    }
}
*/

impl<'tcx> Trans for subst::Substs<'tcx> {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        format!("{} {}",
                self.regions.trans(trcx),
                self.types.as_slice().trans(trcx))
    }
}

impl Trans for subst::RegionSubsts {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        match *self {
            subst::ErasedRegions => panic!("unsupported ErasedRegions"),
            subst::NonerasedRegions(ref regions) => regions.as_slice().trans(trcx),
        }
    }
}

impl Trans for FunctionRetTy {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        match *self {
            Return(ref t) => t.trans(trcx),
            NoReturn(_) => format!("bottom"),
            DefaultReturn(_) => format!("unit"),
        }
    }
}


impl Trans for FnDecl {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        format!("(args {}) return {}",
                self.inputs.trans(trcx),
                self.output.trans(trcx))
    }
}

impl Trans for Arg {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        let name = match self.pat.node {
            PatIdent(_, span_ident, _) => {
                // TODO: check that span_ident doesn't refer to a nullary enum variant
                format!("{}", span_ident.node.as_str())
            },
            _ => panic!("unsupported Pat_ variant in Arg"),
        };
        format!("{} {}", name, self.ty.trans(trcx))
    }
}

impl Trans for Ty {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        match trcx.tcx.ast_ty_to_ty_cache.borrow().get(&self.id) {
            Some(&ty::atttce_resolved(t)) => t.trans(trcx),
            //_ => panic!("no ast_ty_to_ty_cache entry for {}", self),
            _ => format!("[[no_ty_to_ty {}]]", self.repr(trcx.tcx)),
        }
    }
}

impl<'tcx> Trans for ty::Ty<'tcx> {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        use rustc::middle::ty::sty::*;
        let s = match self.sty {
            ty_bool => format!("bool"),
            ty_char => format!("char"),
            ty_int(ity) => format!("int {}",
                                   match ity {
                                       TyI64 => 64us,
                                       TyI32 | TyIs(_) => 32,
                                       TyI16 => 16,
                                       TyI8 => 8,
                                   }),
            ty_uint(uty) => format!("uint {}",
                                    match uty {
                                        TyU64 => 64us,
                                        TyU32 | TyUs(_) => 32,
                                        TyU16 => 16,
                                        TyU8 => 8,
                                    }),
            ty_float(fty) => format!("float {}",
                                     match fty {
                                         TyF64 => 64us,
                                         TyF32 => 32,
                                     }),
            // TODO: handle substs
            ty_enum(did, ref substs) => format!("adt {} {}",
                                                mangled_def_name(trcx, did),
                                                substs.trans(trcx)),
            // ty_uniq
            ty_str => format!("str"),
            ty_vec(ref ty, None) => format!("vec {}",
                                            ty.trans(trcx)),
            ty_vec(ref ty, Some(len)) => format!("fixed_vec {} {}",
                                                 len,
                                                 ty.trans(trcx)),
            ty_ptr(mt) => format!("{} {}",
                                  match mt.mutbl {
                                      MutMutable => "ptr_mut",
                                      MutImmutable => "ptr",
                                  },
                                  mt.ty.trans(trcx)),
            ty_rptr(ref r, mt) => format!("{} {} {}",
                                          match mt.mutbl {
                                              MutMutable => "ref_mut",
                                              MutImmutable => "ref",
                                          },
                                          r.trans(trcx),
                                          mt.ty.trans(trcx)),
            //ty_bare_fn(_, _) => format!("fn"),
            // ty_closure
            // ty_trait
            // TODO: handle substs
            ty_struct(did, ref substs) => format!("adt {} {}",
                                                  mangled_def_name(trcx, did),
                                                  substs.trans(trcx)),
            // ty_unboxed_closure
            ty_tup(ref ts) if ts.len() == 0 => format!("unit"),
            ty_tup(ref ts) => format!("tuple {}", ts.trans(trcx)),
            ty_projection(ref proj) => {
                let trait_did = proj.trait_ref.def_id;
                let name = format!("{}_{}",
                                   mangled_def_name(trcx, proj.trait_ref.def_id),
                                   proj.item_name.trans(trcx));

                trcx.observed_abstract_types.insert(name.clone(), trait_did);

                format!("abstract {} {}",
                        name,
                        proj.trait_ref.substs.trans(trcx))
            },
            ty_param(ref param) => {
                format!("var {}{}",
                        param.space.trans(trcx),
                        param.idx)
            },
            // ty_open
            // ty_infer
            // ty_err
            _ => panic!("unrecognized type: {:?}", self),

        };
        format!("[{}]", s)
    }
}

impl Trans for ty::Region {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        match *self {
            ty::ReEarlyBound(id, _space, _idx, _name) => {
                format!("r_named_0_{}", id)
                //format!("r{}{}", space.trans(trcx), idx)
            },
            ty::ReLateBound(db_idx, ref br) => {
                br.trans_extra(trcx, None)
            },
            ty::ReFree(ref fr) => fr.bound_region.trans_extra(trcx, None),
            ty::ReStatic => format!("r_static"),
            ty::ReScope(extent) => {
                let region::CodeExtent::Misc(id) = extent;
                format!("r_scope_{}", id)
            },
            _ => panic!("unsupported Region variant"),
        }
    }
}

impl TransExtra<Option<NodeId>> for ty::BoundRegion {
    fn trans_extra(&self, trcx: &mut TransCtxt, binder_id: Option<NodeId>) -> String {
        match *self {
            ty::BrAnon(idx) => format!("r_anon_{}", idx),
            ty::BrNamed(did, _) =>
                format!("r_named_{}_{}", did.krate, did.node),
            /*
            ty::BrNamed(did, _) => {
                use syntax::ast_map::Node::*;
                // We know the region is in the function space.  We just need to find the index.
                match trcx.tcx.map.get(binder_id.expect("missing binder_id for BrNamed")) {
                    NodeItem(item) => println!("got item"),
                    NodeTraitItem(item) => println!("got trait item"),
                    NodeImplItem(item) => println!("got impl item"),
                    _ => println!("got other item!!"),
                }
                "[[BrNamed]]".into_string()
            },
            */
            _ => panic!("unsupported BoundRegion variant"),
        }
    }
}

impl Trans for Block {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        format!("{} {{\n{}\t{}\n}}\n",
                match self.rules {
                    DefaultBlock => "block",
                    UnsafeBlock(_) => "unsafe",
                },
                self.stmts.trans(trcx),
                self.expr.as_ref().map(|e| e.trans(trcx))
                    .unwrap_or(format!("[unit] simple_literal _Block")))
    }
}

impl Trans for Stmt {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        match self.node {
            StmtDecl(ref d, _id) => format!("\t{};\n", d.trans(trcx)),
            StmtExpr(ref e, _id) => format!("\texpr {};\n", e.trans(trcx)),
            StmtSemi(ref e, _id) => format!("\texpr {};\n", e.trans(trcx)),
            StmtMac(..) => panic!("expected no macros, but saw StmtMac"),
        }
    }
}

impl Trans for Decl {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        match self.node {
            DeclLocal(ref local) => local.trans(trcx),
            // TODO: handle inner items
            DeclItem(_) => format!("expr ([unit] simple_literal _DeclItem)"),
        }
    }
}

impl Trans for Local {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        let name = match self.pat.node {
            PatIdent(_, span_ident, _) => {
                // TODO: check that span_ident doesn't refer to a nullary enum variant
                format!("{}", span_ident.node.as_str())
            },
            _ => panic!("unsupported Pat_ variant in Local"),
        };
        assert!(self.init.is_some());
        format!("let {} {} {}",
                name,
                trcx.tcx.node_types.borrow()[self.id].trans(trcx),
                self.init.as_ref().unwrap().trans(trcx))
    }
}

impl Trans for Field {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        format!("{} {}",
                self.ident.node.trans(trcx),
                self.expr.trans(trcx))
    }
}

impl Trans for Lifetime {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        use rustc::middle::resolve_lifetime::DefRegion::*;
        match *trcx.tcx.named_region_map.get(&self.id)
                  .expect("missing DefRegion") {
            DefStaticRegion => format!("r_static"),
            DefEarlyBoundRegion(_, _, id) |
            DefLateBoundRegion(_, id) =>
                format!("r_named_{}_{}", LOCAL_CRATE, id),
            DefFreeRegion(..) => panic!("unsupported DefFreeRegion"),
        }
    }
}

fn trans_method_call(trcx: &mut TransCtxt,
                     callee: &MethodCallee,
                     args: Vec<String>) -> String {
    let name = match callee.origin {
        MethodOrigin::MethodStatic(did) => {
            mangled_def_name(trcx, did)
        },
        MethodOrigin::MethodTypeParam(ref mp) => {
            let trait_did = mp.trait_ref.def_id;
            // trait_ref substs are actually the same as the callee substs, so we can
            // ignore them here.
            let item_id = trcx.tcx.trait_item_def_ids.borrow()[trait_did][mp.method_num];
            let method_did = match item_id {
                ty::ImplOrTraitItemId::MethodTraitItemId(did) => did,
                ty::ImplOrTraitItemId::TypeTraitItemId(_) =>
                    panic!("unexpected TypeTraitItemId in method call"),
            };
            let name = mangled_def_name(trcx, method_did);
            trcx.observed_abstract_fns.insert(name.clone(), method_did);
            name
        },
        _ => panic!("unsupported MethodOrigin variant"),
    };
    format!("call {} {} {}",
            name,
            callee.substs.trans(trcx),
            args.trans(trcx))
}

impl Trans for Expr {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        let mut add_ty = true;

        let variant = match self.node {
            // ExprBox
            // ExprVec
            ExprCall(ref func, ref args) => {
                if let Some((var_name, var_idx)) = find_variant(trcx, func.id) {
                    format!("enum_literal {} {} {}",
                            var_name,
                            var_idx,
                            args.trans(trcx))
                } else {
                    let (did, is_abstract) = match trcx.tcx.def_map.borrow()[func.id] {
                        def::DefStaticMethod(did, prov) => match prov {
                            def::MethodProvenance::FromTrait(_) => (did, true),
                            _ => (did, false),
                        },
                        def => (def.def_id(), false),
                    };
                    let name = mangled_def_name(trcx, did);
                    let substs = match trcx.tcx.item_substs.borrow().get(&func.id) {
                        Some(item_substs) => item_substs.substs.trans(trcx),
                        None => format!("0 0"),
                    };
                    if is_abstract {
                        trcx.observed_abstract_fns.insert(name.clone(), did);
                    }
                    format!("call {} {} {}",
                            name,
                            substs,
                            args.trans(trcx))
                }
            },
            ExprMethodCall(name, ref tys, ref args) => {
                let call = MethodCall::expr(self.id);
                let map = trcx.tcx.method_map.borrow();
                let callee = &map[call];
                let arg_strs = args.iter().map(|x| x.trans(trcx)).collect();
                assert!(tys.len() == 0); // no idea what `tys` does
                trans_method_call(trcx, callee, arg_strs)
            },
            ExprTup(ref xs) if xs.len() == 0 => format!("simple_literal _"),
            ExprTup(ref xs) => format!("tuple_literal {}", xs.trans(trcx)),
            ExprBinary(op, ref a, ref b) => {
                match trcx.tcx.method_map.borrow().get(&MethodCall::expr(self.id)) {
                    Some(callee) => {
                        let arg_strs = vec![a.trans(trcx), b.trans(trcx)];
                        trans_method_call(trcx, callee, arg_strs)
                    },
                    None => {
                        format!("binop {:?} {} {}",
                                op.node,
                                a.trans(trcx),
                                b.trans(trcx))
                    },
                }
            },
            ExprUnary(op, ref a) => {
                match trcx.tcx.method_map.borrow().get(&MethodCall::expr(self.id)) {
                    Some(callee) => {
                        let arg_strs = vec![a.trans(trcx)];
                        trans_method_call(trcx, callee, arg_strs)
                    },
                    None => {
                        match op {
                            UnDeref => format!("deref {}", a.trans(trcx)),
                            _ => format!("unop {:?} {}",
                                         op,
                                         a.trans(trcx)),
                        }
                    },
                }
            },
            ExprLit(ref lit) =>
                format!("simple_literal {}", lit.trans(trcx)),
            ExprCast(ref e, ref ty) =>
                format!("cast {} {}",
                        e.trans(trcx),
                        ty.trans(trcx)),
            ExprIf(ref cond, ref then, ref opt_else) => {
                let ty = trcx.tcx.node_types.borrow()[then.id];

                // NB: `then` is a Block, but opt_else is `Option<Expr>`.
                format!("match {} 2 \
                        {{ ([bool] simple_literal true) >> ({} {}) }} \
                        {{ ([bool] simple_literal false) >> {} }}",
                        cond.trans(trcx),
                        ty.trans(trcx),
                        then.trans(trcx),
                        opt_else.as_ref().map_or(format!("[unit] simple_literal _ExprIf"),
                                                 |e| e.trans(trcx)))
            },
                
                        /*
                format!("[[ExprIf {} {} {}]]",
                        cond.trans(trcx),
                        then.trans(trcx),
                        opt_else.as_ref().map(|e| e.trans(trcx))),
                        */
            // ExprIfLet
            // ExprWhile
            // ExprWhileLet
            // ExprForLoop
            // ExprLoop
            ExprMatch(ref expr, ref arms, _src) =>
                format!("match {} {}",
                        expr.trans(trcx),
                        arms.trans(trcx)),
            // ExprFnBlock
            // ExprProc
            // ExprUnboxedFn
            ExprBlock(ref b) => b.trans(trcx),
            ExprAssign(ref l, ref r) =>
                format!("assign {} {}",
                        l.trans(trcx),
                        r.trans(trcx)),
            // ExprAssignOp
            ExprField(ref expr, field) =>
                format!("field {} {}",
                        expr.trans(trcx),
                        field.node.as_str()),
            // ExprTupField
            ExprIndex(ref arr, ref idx) => panic!("exprindex"),
            ExprRange(ref low, ref high) => panic!("exprrange"),
            ExprPath(ref path) => {
                if let Some((var_name, var_idx)) = find_variant(trcx, self.id) {
                    format!("enum_literal {} {} 0",
                            var_name,
                            var_idx)
                } else {
                    use rustc::middle::def::*;
                    match trcx.tcx.def_map.borrow()[self.id] {
                        DefLocal(..) =>
                            format!("var {}",
                                    path.segments[path.segments.len() - 1]
                                        .identifier.as_str()),
                        DefStruct(did) =>
                            format!("struct_literal 0"),
                        d => format!("const {}",
                                     mangled_def_name(trcx, d.def_id())),
                    }
                }
            },
            ExprAddrOf(_mutbl, ref expr) =>
                format!("addr_of {}", expr.trans(trcx)),
            // ExprBreak
            // ExprAgain
            ExprRet(ref opt_expr) =>
                format!("return {}",
                        opt_expr.as_ref().map(|e| e.trans(trcx))
                                .unwrap_or(format!("[unit] simple_literal _ExprRet"))),
            // ExprInlineAsm
            // ExprMac
            ExprStruct(ref name, ref fields, ref opt_base) => {
                assert!(opt_base.is_none());
                format!("struct_literal {}", fields.trans(trcx))
            },
            // ExprRepeat
            ExprParen(ref expr) => {
                add_ty = false;
                expr.trans(trcx)
            },
            _ => panic!("unrecognized Expr_ variant"),
        };

        let expr_ty = trcx.tcx.node_types.borrow()[self.id];
        let unadjusted = 
                if add_ty {
                    format!("({} {})",
                            expr_ty.trans(trcx),
                            variant)
                } else {
                    variant
                };

        match trcx.tcx.adjustments.borrow().get(&self.id) {
            None => unadjusted,
            Some(adj) => adjust_expr(trcx, adj, self, unadjusted),
        }
    }
}

fn adjust_expr(trcx: &mut TransCtxt,
               adj: &ty::AutoAdjustment,
               expr: &Expr,
               unadjusted: String) -> String {
    let mut result = unadjusted;
    let mut result_ty = trcx.tcx.node_types.borrow()[expr.id];

    match *adj {
        ty::AdjustDerefRef(ref adr) => {
            for i in range(0, adr.autoderefs) {
                let (new_result, new_result_ty) = deref_once(trcx, expr, i, result, result_ty);
                result = new_result;
                result_ty = new_result_ty;
            }

            match adr.autoref {
                None => {},
                Some(ty::AutoPtr(region, mutbl, ref autoref)) => {
                    assert!(autoref.is_none());
                    let mt = ty::mt { ty: result_ty, mutbl: mutbl };
                    result_ty = ty::mk_t(trcx.tcx, ty::ty_rptr(trcx.tcx.mk_region(region), mt));
                    result = format!("({} addr_of {})",
                                     result_ty.trans(trcx),
                                     result);
                },
                Some(ty::AutoUnsafe(mutbl, ref autoref)) => {
                    assert!(autoref.is_none());
                    let mt = ty::mt { ty: result_ty, mutbl: mutbl };
                    result_ty = ty::mk_t(trcx.tcx, ty::ty_ptr(mt));
                    result = format!("({} addr_of {})",
                                     result_ty.trans(trcx),
                                     result);
                },
                _ => panic!("unsupported AutoRef variant"),
            }

            //assert!(adr.autoref.is_none());
        },
        ty::AdjustReifyFnPointer(_) => panic!("unsupported AdjustAddEnv"),
    }

    result
}

fn deref_once<'a, 'tcx>(trcx: &mut TransCtxt<'a, 'tcx>,
                   expr: &Expr,
                   level: usize,
                   expr_str: String,
                   expr_ty: ty::Ty<'tcx>) -> (String, ty::Ty<'tcx>) {
    match expr_ty.sty {
        ty::ty_ptr(ty::mt { ty, .. }) |
        ty::ty_rptr(_, ty::mt { ty, .. }) => {
            let new_expr_str = format!("({} deref {})",
                                       ty.trans(trcx),
                                       expr_str);
            (new_expr_str, ty)
        },
        _ => panic!("unexpected ty variant"),
    }
}

fn find_variant(trcx: &mut TransCtxt, id: NodeId) -> Option<(String, usize)> {
    use rustc::middle::def::*;

    let def_map = trcx.tcx.def_map.borrow();

    let def = match def_map.get(&id) {
        None => return None,
        Some(d) => d,
    };

    match *def {
        DefVariant(enum_did, variant_did, _is_structure) => {
            let info = ty::enum_variant_with_id(trcx.tcx, enum_did, variant_did);
            Some((mangled_def_name(trcx, variant_did), info.disr_val as usize))
        },
        _ => None,
    }
}

impl Trans for Lit {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        match self.node {
            // LitStr
            // LitBinary
            LitByte(b) => format!("{}", b),
            // LitChar
            LitInt(i, _) => format!("{}", i),
            // LitFloat
            // LitFloatUnsuffixed
            LitBool(b) => format!("{}", b),
            _ => panic!("unrecognized Lit_ variant"),
        }
    }
}

impl Trans for Arm {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        assert!(self.pats.len() == 1);
        assert!(self.guard.is_none());
        format!("{{ {} >> {} }}",
                self.pats[0].trans(trcx),
                self.body.trans(trcx))
    }
}

impl Trans for Pat {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        let variant = match self.node {
            PatWild(PatWildSingle) => format!("wild"),
            PatIdent(_mode, name, None) => {
                if let Some((var_name, var_idx)) = find_variant(trcx, self.id) {
                    format!("enum {} {} 0",
                            var_name,
                            var_idx)
                } else {
                    use rustc::middle::def::*;
                    match trcx.tcx.def_map.borrow().get(&self.id) {
                        None | Some(&DefLocal(_)) => format!("var {}",
                                                             name.node.trans(trcx)),
                        Some(ref d) => format!("const {}",
                                               mangled_def_name(trcx, d.def_id())),
                    }
                }
            },
            PatEnum(ref path, Some(ref args)) => {
                let (var_name, var_idx) = find_variant(trcx, self.id)
                        .expect("couldn't find variant for enum pattern");
                format!("enum {} {} {}",
                        var_name,
                        var_idx,
                        args.trans(trcx))
            },
            PatTup(ref args) => format!("tuple {}", args.trans(trcx)),
            // NB: For PatLit, we skip the code below that adds the pattern type.
            PatLit(ref expr) => return expr.trans(trcx),
            _ => panic!("unhandled Pat_ variant"),
        };
        format!("({} {})",
                trcx.tcx.node_types.borrow()[self.id].trans(trcx),
                variant)
    }
}

impl Trans for StructField {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        format!("{} {}",
                self.node.ident().unwrap().as_str(),
                self.node.ty.trans(trcx))
    }
}

impl Trans for Variant {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        format!("{} {}",
                mangled_def_name(trcx, local_def(self.node.id)),
                self.node.kind.trans(trcx))
    }
}

impl Trans for VariantKind {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        match *self {
            TupleVariantKind(ref args) => args.trans(trcx),
            _ => panic!("unsupported VariantKind variant"),
        }
    }
}

impl Trans for ty::DtorKind {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        let opt = match *self {
            ty::NoDtor => None,
            ty::TraitDtor(did, _) => Some(mangled_def_name(trcx, did)),
        };
        opt.trans(trcx)
    }
}

impl Trans for VariantArg {
    fn trans(&self, trcx: &mut TransCtxt) -> String {
        self.ty.trans(trcx)
    }
}

struct TransVisitor<'b, 'a: 'b, 'tcx: 'a> {
    trcx: &'b mut TransCtxt<'a, 'tcx>,
    filter_fn: HashSet<String>
}

fn try_str<F: FnOnce() -> String>(f: F, what: &str) -> String {
    let mut opt_str = None;
    let mut opt_f = Some(f);
    let result = unsafe {
        ::std::rt::unwind::try(|| {
            let f = opt_f.take().unwrap();
            opt_str = Some(f());
        })
    };
    match result {
        Ok(()) => {
            opt_str.unwrap()
        },
        Err(e) => {
            fn read(mut e: Box<Any>) -> String {
                match e.downcast::<String>() {
                    Ok(msg) => return *msg,
                    Err(e_) => e = e_,
                }

                match e.downcast::<&'static str>() {
                    Ok(msg) => return String::from_str(*msg),
                    Err(e_) => e = e_,
                }

                format!("(unknown error type: {:?}", e.get_type_id())
            }
            format!("# error with {}: {}", what, read(e))
        },
    }
}

impl<'b, 'a, 'tcx, 'v> Visitor<'v> for TransVisitor<'b, 'a, 'tcx> {
    fn visit_item(&mut self, i: &'v Item) {
        let name = mangled_def_name(self.trcx, local_def(i.id));
        let s = try_str(|| i.trans_extra(self.trcx, &self.filter_fn),
                        &*name);
        println!("{}", s);
        visit::walk_item(self, i);
    }
}

impl<'a> TransExtra<&'a HashSet<String>> for Item {
    fn trans_extra(&self, trcx: &mut TransCtxt, filter_fn: &'a HashSet<String>) -> String {
        match self.node {
            ItemStruct(ref def, ref g) => {
                //assert!(def.ctor_id.is_none());
                format!("struct {} {} {} {};",
                        mangled_def_name(trcx, local_def(self.id)),
                        g.trans_extra(trcx, TypeSpace),
                        def.fields.trans(trcx),
                        ty::ty_dtor(trcx.tcx, local_def(self.id)).trans(trcx))
            },
            ItemEnum(ref def, ref g) => {
                format!("enum {} {} {} {};",
                        mangled_def_name(trcx, local_def(self.id)),
                        g.trans_extra(trcx, TypeSpace),
                        def.variants.trans(trcx),
                        ty::ty_dtor(trcx.tcx, local_def(self.id)).trans(trcx))
            },
            ItemFn(ref decl, style, _, ref generics, ref body) => {
                let mangled_name = mangled_def_name(trcx, local_def(self.id));
                if filter_fn.contains(&mangled_name) {
                    format!("")
                } else {
                    format!("fn {} {} {} 0 body {} {} {{\n{}\t{}\n}}\n\n",
                            mangled_name,
                            generics.trans_extra(trcx, FnSpace),
                            decl.trans(trcx),
                            decl.output.trans(trcx),
                            match style {
                                Unsafety::Unsafe => "unsafe",
                                Unsafety::Normal => "block",
                            },
                            body.stmts.trans(trcx),
                            body.expr.as_ref().map(|e| e.trans(trcx))
                                .unwrap_or(format!("[unit] simple_literal _ItemFn")))
                }
            },
            ItemImpl(_, _, ref impl_generics, ref trait_ref, ref self_ty, ref items) => {
                let mut result = String::new();
                for item in items.iter() {
                    let part = match *item {
                        MethodImplItem(ref method) => {
                            let name = mangled_def_name(trcx, local_def(method.id));
                            try_str(|| trans_method(trcx, self, &**method), &*name)
                        },
                        TypeImplItem(ref td) => {
                            let name = mangled_def_name(trcx, local_def(td.id));
                            try_str(|| {
                                let name_str = td.ident.trans(trcx);
                                let self_str = self_ty.trans(trcx);
                                let typ_str = td.typ.trans(trcx);
                                format!("associated_type {} {} {}",
                                        impl_generics.trans_extra(trcx, TypeSpace),
                                        trans_impl_clause(trcx,
                                                          trait_ref.as_ref().unwrap(),
                                                          name_str,
                                                          self_str),
                                        typ_str)
                            }, &*name)
                        },
                    };
                    result.push_str(part.as_slice());
                    result.push_str("\n");
                }
                result
            },
            ItemConst(ref ty, ref expr) => {
                format!("const {} {} {}",
                        mangled_def_name(trcx, local_def(self.id)),
                        ty.trans(trcx),
                        expr.trans(trcx))
            },
            ItemForeignMod(ref fm) => {
                let abi_str = format!("{:?}", fm.abi);
                let mut result = String::new();
                for item in fm.items.iter() {
                    let part = match item.node {
                        ForeignItemFn(ref decl, ref generics) => {
                            let name = mangled_def_name(trcx, local_def(item.id));
                            try_str(|| {
                                format!("extern_fn {} {} {} {}",
                                        abi_str,
                                        name,
                                        generics.trans_extra(trcx, FnSpace),
                                        decl.trans(trcx))
                            }, &*name)
                        },
                        ForeignItemStatic(ref ty, is_mutbl) => {
                            let name = mangled_def_name(trcx, local_def(item.id));
                            try_str(|| {
                                panic!("can't translate ForeignItemStatic");
                            }, &*name)
                        },
                    };
                    result.push_str(part.as_slice());
                    result.push_str("\n");
                }
                result

            },
            _ => format!(""),
        }
    }
}

fn combine_generics(trcx: &mut TransCtxt, impl_g: &Generics, fn_g: &Generics) -> (Vec<String>, Vec<String>) {
    let lifetimes =
            impl_g.lifetimes.iter().map(|l| format!("r_named_0_{}", l.lifetime.id)).chain(
            fn_g.lifetimes.iter().map(|l| format!("r_named_0_{}", l.lifetime.id))).collect();
    let ty_params =
            range(0, impl_g.ty_params.len()).map(|i| format!("t_{}", i)).chain(
            range(0, fn_g.ty_params.len()).map(|i| format!("f_{}", i))).collect();
    (lifetimes, ty_params)
}

fn clean_path_elem(s: &str, out: &mut String) {
    let mut depth = 0us;
    for c in s.chars() {
        if c == '<' {
            depth += 1;
        } else if c == '>' {
            depth -= 1;
        } else if depth == 0 {
            if c == '.' {
                out.push_str("__");
            } else {
                out.push(c);
            }
        }
    }
}

fn mangled_def_name(trcx: &mut TransCtxt, did: DefId) -> String {
    let mut name = String::new();
    if did.krate == LOCAL_CRATE {
        name.push_str(&*trcx.crate_name);
        name.push_str("_");
        trcx.tcx.map.with_path(did.node, |mut elems| {
            for elem in elems {
                clean_path_elem(elem.name().as_str(), &mut name);
                name.push_str("_");
            }
        })
    } else {
        for elem in csearch::get_item_path(trcx.tcx, did).into_iter() {
            clean_path_elem(elem.name().as_str(), &mut name);
            name.push_str("_");
        }
    }
    name.pop();

    sanitize_ident(&*name)
}

fn sanitize_ident(s: &str) -> String {
    let mut last_i = 0;
    let mut result = String::with_capacity(s.len());
    for (i, c) in s.chars().enumerate() {
        if (c >= '0' && c <= '9') || (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || c == '_' {
            continue;
        }
        result.push_str(s.slice(last_i, i));

        let n = c as u32;
        if n <= 0xff {
            result.push_str(&*format!("_x{:02x}", n));
        } else if n <= 0xffff {
            result.push_str(&*format!("_u{:04x}", n));
        } else {
            result.push_str(&*format!("_U{:08x}", n));
        }
        last_i = i + 1;
    }
    result.push_str(s.slice_from(last_i));
    result
}

/*
fn find_item_ast<'tcx>(tcx: &ty::ctxt<'tcx>, did: DefId) -> Option<&'tcx Item> {
    if did.krate == LOCAL_CRATE {
        match tcx.map.get(did.node) {
            ast_map::NodeItem(ast) => Some(ast),
            _ => panic!("expected NodeItem"),
        }
    } else {
        let result = csearch::maybe_get_item_ast(
            tcx, did,
            |a,b,c,d| astencode::decode_inlined_item(a, b, c, d));
        let item = match result {
            csearch::not_found => return None,
            csearch::found(item) => item,
            csearch::found_parent(_, item) => item,
        };
        match item {
            &IIItem(ref ast) => Some(&**ast),
            _ => panic!("expected IIItem"),
        }
    }
}

fn find_trait_item_ast<'tcx>(tcx: &ty::ctxt<'tcx>, did: DefId) -> Option<&'tcx TraitItem> {
    if did.krate == LOCAL_CRATE {
        match tcx.map.get(did.node) {
            ast_map::NodeTraitItem(ast) => Some(ast),
            _ => panic!("expected NodeTraitItem"),
        }
    } else {
        let result = csearch::maybe_get_item_ast(
            tcx, did,
            |a,b,c,d| astencode::decode_inlined_item(a, b, c, d));
        let item = match result {
            csearch::not_found => return None,
            csearch::found(item) => item,
            csearch::found_parent(_, item) => item,
        };
        match item {
            &IITraitItem(_, ref ast) => Some(ast),
            _ => panic!("expected IITraitItem"),
        }
    }
}
*/

fn trans_method(trcx: &mut TransCtxt, trait_: &Item, method: &Method) -> String {
    let mangled_name = mangled_def_name(trcx, local_def(method.id));

    /*
    if filter_fn.contains(&mangled_name) {
        return format!("");
    };
    */

    let (impl_generics, trait_ref, self_ty, items) = match trait_.node {
        ItemImpl(_, _, ref a, ref b, ref c, ref d) => (a, b, c, d),
        _ => panic!("expected ItemImpl"),
    };

    let (name, generics, _, exp_self, style, decl, body, _) = match method.node {
        MethDecl(a, ref b, c, ref d, e, ref f, ref g, h) => (a, b, c, d, e, f, g, h),
        MethMac(_) => panic!("unexpected MethMac"),
    };

    let mut arg_strs = vec![];



    let self_arg = match exp_self.node {
        SelfStatic => None,
        SelfValue(ref name) =>
            Some(format!("{} {}",
                         name.trans(trcx),
                         self_ty.trans(trcx))),
        SelfRegion(ref opt_lifetime, mutbl, ref name) =>
            Some(format!("{} {} {} {}",
                         name.trans(trcx),
                         match mutbl {
                             MutMutable => "ref_mut",
                             MutImmutable => "ref",
                         },
                         match *opt_lifetime {
                             Some(ref lifetime) =>
                                 lifetime.trans(trcx),
                             None => format!("r_anon_0"),
                         },
                         self_ty.trans(trcx))),
        SelfExplicit(ref ty, ref name) =>
            Some(format!("{} {}",
                         name.trans(trcx),
                         self_ty.trans(trcx))),
    };
    let offset = match self_arg {
        Some(arg) => {
            arg_strs.push(arg);
            1
        },
        None => 0,
    };

    let impl_clause = match *trait_ref {
        Some(ref trait_ref) => {
            let name_str = name.trans(trcx);
            let self_str = self_ty.trans(trcx);
            format!("1 {}",
                    trans_impl_clause(trcx,
                                      trait_ref,
                                      name_str,
                                      self_str))
        },
        None => format!("0"),
    };

    let (lifetimes, ty_params) = combine_generics(trcx, impl_generics, generics);

    arg_strs.extend(decl.inputs.slice_from(offset).iter().map(|x| x.trans(trcx)));
    format!("fn {} {} {} (args {}) return {} {} body {} {} {{\n{}\t{}\n}}\n\n",
            mangled_name,
            lifetimes.trans(trcx),
            ty_params.trans(trcx),
            arg_strs.trans(trcx),
            decl.output.trans(trcx),
            impl_clause,
            decl.output.trans(trcx),
            match style {
                Unsafety::Unsafe => "unsafe",
                Unsafety::Normal => "block",
            },
            body.stmts.trans(trcx),
            body.expr.as_ref().map(|e| e.trans(trcx))
                .unwrap_or(format!("[unit] simple_literal _method")))
}

fn trans_impl_clause(trcx: &mut TransCtxt,
                     trait_ref: &TraitRef,
                     name: String,
                     self_ty: String) -> String {
    let last_seg = trait_ref.path.segments.as_slice().last().unwrap();
    let mut tys = vec![];
    let mut lifes = vec![];
    match last_seg.parameters {
        AngleBracketedParameters(ref params) => {
            for life in params.lifetimes.iter() {
                lifes.push(life.trans(trcx));
            }
            for ty in params.types.iter() {
                tys.push(ty.trans(trcx));
            }
        },
        ParenthesizedParameters(_) =>
            panic!("unsupported ParenthesizedParameters"),
    }
    tys.push(self_ty.trans(trcx));

    format!("{}_{} {} {}",
            mangled_def_name(trcx, trcx.tcx.def_map.borrow()[trait_ref.ref_id].def_id()),
            name.trans(trcx),
            lifes.trans(trcx),
            tys.trans(trcx))
}

fn print_abstract_fn_decls(trcx: &mut TransCtxt) {
    let mut names = trcx.observed_abstract_fns.iter()
                        .map(|(k,v)| (k.clone(), v.clone()))
                        .collect::<Vec<_>>();
    names.sort();
    let names = names;


    for (name, method_did) in names.into_iter() {
        let (method_generics, inputs, output) = {
            let trait_defs = trcx.tcx.trait_defs.borrow();
            let impl_or_trait_items = trcx.tcx.impl_or_trait_items.borrow();
            let opt_method = impl_or_trait_items.get(&method_did);

            let method = match opt_method {
                None => panic!("can't find method for {}", name),
                Some(x) => match x {
                    &ty::MethodTraitItem(ref m) => &**m,
                    _ => panic!("expected MethodTraitItem"),
                }
            };

            (method.generics.clone(),
             method.fty.sig.0.inputs.clone(),
             method.fty.sig.0.output.clone())
        };

        let mut args = Vec::new();
        for (i, arg) in inputs.iter().enumerate() {
            args.push(format!("arg{} {}", i, arg.trans(trcx)));
        }

        let mut regions = Vec::new();
        for region in method_generics.regions.iter() {
            regions.push(format!("{}{}",
                                 region.space.trans(trcx),
                                 region.index));
        }

        let mut types = Vec::new();
        for ty_param in method_generics.types.iter() {
            types.push(format!("{}{}",
                               ty_param.space.trans(trcx),
                               ty_param.index));
        }

        let return_ty = match output {
            ty::FnConverging(ty) => ty.trans(trcx),
            ty::FnDiverging => format!("bottom"),
        };

        println!("abstract_fn {} {} {} args {} return {}",
                 name,
                 regions.trans(trcx),
                 types.trans(trcx),
                 args.trans(trcx),
                 return_ty);
    }
}

fn print_abstract_type_decls(trcx: &mut TransCtxt) {
    let mut names = trcx.observed_abstract_types.iter()
                        .map(|(k,v)| (k.clone(), v.clone()))
                        .collect::<Vec<_>>();
    names.sort();
    let names = names;

    for (name, trait_did) in names.into_iter() {
        let trait_generics = {
            let trait_defs = trcx.tcx.trait_defs.borrow();
            let opt_trait = trait_defs.get(&trait_did);
            let trait_ = match opt_trait {
                None => panic!("can't find trait for {}", name),
                Some(ref t) => &**t,
            };

            trait_.generics.clone()
        };

        let mut regions = Vec::new();
        for region in trait_generics.regions.iter() {
            regions.push(format!("{}{}",
                                 region.space.trans(trcx),
                                 region.index));
        }

        let mut types = Vec::new();
        for ty_param in trait_generics.types.iter() {
            types.push(format!("{}{}",
                               ty_param.space.trans(trcx),
                               ty_param.index));
        }

        println!("abstract_type {} {} {}",
                 name,
                 regions.trans(trcx),
                 types.trans(trcx));
    }
}


pub fn process(tcx: &ty::ctxt, filter_fn : HashSet<String>, crate_name: String) {
    let krate = tcx.map.krate();
    let mut trcx = TransCtxt {
        tcx: tcx,
        observed_abstract_fns: HashMap::new(),
        observed_abstract_types: HashMap::new(),
        crate_name: crate_name,
    };
    {
        let mut visitor = TransVisitor { trcx: &mut trcx, filter_fn: filter_fn };
        visit::walk_crate(&mut visitor, krate);
    }
    print_abstract_fn_decls(&mut trcx);
    print_abstract_type_decls(&mut trcx);
}
