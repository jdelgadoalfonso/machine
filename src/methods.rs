use std::collections::HashMap;

use syn::{
    Abi, Expr, FnArg, FnDecl, Generics, Ident, MethodSig, ReturnType, Type, WhereClause,
    export::Span,
    parse::{Parse, ParseStream, Result},
    punctuated::Punctuated,
};

#[derive(Debug)]
pub struct Methods {
    pub machine_name: Ident,
    pub methods: Vec<Method>,
}

#[derive(Debug)]
pub struct Method {
    pub states: Vec<Ident>,
    pub method_type: MethodType,
    pub default: DefaultValue,
}

#[derive(Debug)]
pub enum MethodType {
    Get(Ident, Type),
    Set(Ident, Type),
    Fn(MethodSig),
}

#[derive(Debug)]
pub enum DefaultValue {
    None,
    Default,
    Val(Expr),
}

impl DefaultValue {
    pub fn is_default(&self) -> bool {
        match self {
            DefaultValue::None => false,
            _ => true,
        }
    }
}

struct ParenVal {
    expr: Expr,
}

impl Parse for ParenVal {
    fn parse(input: ParseStream) -> Result<Self> {
        let stream;
        parenthesized!(stream in input);
        let expr: Expr = stream.parse()?;
        Ok(ParenVal { expr })
    }
}

impl Parse for Methods {
    fn parse(input: ParseStream) -> Result<Self> {
        let machine_name: Ident = input.parse()?;
        let _: Token![,] = input.parse()?;

        let content;
        bracketed!(content in input);

        let mut methods = Vec::new();

        let t: Method = content.parse()?;
        methods.push(t);

        loop {
            let lookahead = content.lookahead1();
            if lookahead.peek(Token![,]) {
                let _: Token![,] = content.parse()?;
                let t: Method = content.parse()?;
                methods.push(t);
            } else {
                break;
            }
        }

        Ok(Methods {
            machine_name,
            methods,
        })
    }
}

impl Parse for Method {
    fn parse(input: ParseStream) -> Result<Self> {
        let states: Vec<_> = match input.parse::<Ident>() {
            Ok(i) => vec![i],
            Err(_) => {
                let content;
                bracketed!(content in input);

                let punctuated: Punctuated<Ident, Token![,]> =
                    content.parse_terminated(Ident::parse)?;

                let states: Vec<_> = punctuated.into_iter().collect();

                states
            }
        };

        let _: Token![=>] = input.parse()?;
        let default_token: Option<Token![default]> = input.parse()?;
        let default = if default_token.is_some() {
            match input.parse::<ParenVal>() {
                Ok(content) => DefaultValue::Val(content.expr),
                Err(_) => DefaultValue::Default,
            }
        } else {
            DefaultValue::None
        };

        let method_type = match parse_method_sig(input) {
            Ok(f) => MethodType::Fn(f),
            Err(_) => {
                let i: Ident = input.parse()?;
                let name: Ident = input.parse()?;
                let _: Token![:] = input.parse()?;
                let ty: Type = input.parse()?;

                if i.to_string() == "get" {
                    MethodType::Get(name, ty)
                } else if i.to_string() == "set" {
                    MethodType::Set(name, ty)
                } else {
                    return Err(syn::Error::new(i.span(), "expected `get` or `set`"));
                }
            }
        };

        Ok(Method {
            states,
            method_type,
            default,
        })
    }
}

fn parse_method_sig(input: ParseStream) -> Result<MethodSig> {
    //let vis: Visibility = input.parse()?;
    let constness: Option<Token![const]> = input.parse()?;
    let unsafety: Option<Token![unsafe]> = input.parse()?;
    let asyncness: Option<Token![async]> = input.parse()?;
    let abi: Option<Abi> = input.parse()?;
    let fn_token: Token![fn] = input.parse()?;
    let ident: Ident = input.parse()?;
    let generics: Generics = input.parse()?;

    let content;
    let paren_token = parenthesized!(content in input);
    let inputs = content.parse_terminated(FnArg::parse)?;

    let output: ReturnType = input.parse()?;
    let where_clause: Option<WhereClause> = input.parse()?;

    Ok(MethodSig {
        constness,
        unsafety,
        asyncness,
        abi,
        ident,
        decl: FnDecl {
            fn_token: fn_token,
            paren_token: paren_token,
            inputs: inputs,
            output: output,
            variadic: None,
            generics: Generics {
                where_clause: where_clause,
                ..generics
            },
        },
    })
}

impl Methods {
    pub fn generate(&self) -> (&Ident, syn::export::TokenStream) {
        let machine_name = &self.machine_name;
        let mut stream = proc_macro::TokenStream::new();

        stream.extend(self.generate_state_impls());
        stream.extend(self.generate_impl());

        (machine_name, stream)
    }

    pub fn generate_state_impls(&self) -> syn::export::TokenStream {
        let mut stream = proc_macro::TokenStream::new();

        let mut h = HashMap::new();
        for method in self.methods.iter() {
            for state in method.states.iter() {
                let entry = h.entry(state).or_insert(Vec::new());
                entry.push(&method.method_type);
            }
        }

        for (state, methods) in h.iter() {
            stream.extend(self.generate_state_impl(*state, methods));
        }

        stream
    }

    pub fn generate_state_impl(
        &self,
        state: &syn::Ident,
        methods: &[&MethodType],
    ) -> syn::export::TokenStream {
        let method_tokens = methods
            .iter()
            .map(|method| {
                match method {
                    MethodType::Get(ident, ty) => {
                        quote! {
                          pub fn #ident(&self) -> &#ty {
                            &self.#ident
                          }
                        }
                    }
                    MethodType::Set(ident, ty) => {
                        let mut_ident =
                            Ident::new(&format!("{}_mut", &ident.to_string()), Span::call_site());
                        quote! {
                          pub fn #mut_ident(&mut self) -> &mut #ty {
                            &mut self.#ident
                          }
                        }
                    }
                    MethodType::Fn(_) => {
                        // we let the user implement these methods on the types
                        quote! {}
                    }
                }
            })
            .collect::<Vec<_>>();

        let tokens = quote! {
            impl #state {
                #(#method_tokens)*
            }
        };

        proc_macro::TokenStream::from(tokens)
    }

    pub fn generate_impl(&self) -> syn::export::TokenStream {
        let machine_name = &self.machine_name;

        let wrapper_methods = self
            .methods
            .iter()
            .map(|method| match &method.method_type {
                MethodType::Get(ident, ty) => self.generate_getter(method, ident, ty),
                MethodType::Set(ident, ty) => self.generate_setter(method, ident, ty),
                MethodType::Fn(signature) => self.generate_fn(method, signature),
            })
            .collect::<Vec<_>>();

        let tokens = quote! {
            impl #machine_name {
                #(#wrapper_methods)*
            }
        };

        proc_macro::TokenStream::from(tokens)
    }

    fn generate_getter(
        &self,
        method: &Method,
        ident: &syn::Ident,
        ty: &syn::Type,
    ) -> syn::export::TokenStream2 {
        let machine_name = &self.machine_name;

        let variants = method
            .states
            .iter()
            .map(|state| {
                quote! {
                    #machine_name::#state(ref v) => Some(v.#ident()),
                }
            })
            .collect::<Vec<_>>();

        let tokens = quote! {
            pub fn #ident(&self) -> Option<&#ty> {
                match self {
                    #(#variants)*
                    _ => None,
                }
            }
        };

        tokens
    }

    fn generate_setter(
        &self,
        method: &Method,
        ident: &syn::Ident,
        ty: &syn::Type,
    ) -> syn::export::TokenStream2 {
        let machine_name = &self.machine_name;

        let mut_ident = Ident::new(&format!("{}_mut", &ident.to_string()), Span::call_site());

        let variants = method
            .states
            .iter()
            .map(|state| {
                quote! {
                    #machine_name::#state(ref mut v) => Some(v.#mut_ident()),
                }
            })
            .collect::<Vec<_>>();

        let tokens = quote! {
            pub fn #mut_ident(&mut self) -> Option<&mut #ty> {
                match self {
                    #(#variants)*
                    _ => None,
                }
            }
        };

        tokens
    }

    fn generate_fn(
        &self,
        method: &Method,
        signature: &syn::MethodSig,
    ) -> syn::export::TokenStream2 {
        let machine_name = &self.machine_name;

        let ident = &signature.ident;
        let args = signature
            .decl
            .inputs
            .iter()
            .filter(|arg| match arg {
                FnArg::Captured(_) => true,
                _ => false,
            })
            .map(|arg| {
                if let FnArg::Captured(a) = arg {
                    &a.pat
                } else {
                    panic!();
                }
            })
            .collect::<Vec<_>>();

        let variants = method
            .states
            .iter()
            .map(|state| {
                let a = args.clone();
                if method.default.is_default() {
                    quote! {
                        #machine_name::#state(ref v) => v.#ident( #(#a),* ),
                    }
                } else {
                    quote! {
                        #machine_name::#state(ref v) => Some(v.#ident( #(#a),* )),
                    }
                }
            })
            .collect::<Vec<_>>();

        let inputs = &signature.decl.inputs;
        let output = match &signature.decl.output {
            ReturnType::Default => quote! {},
            ReturnType::Type(arrow, ty) => {
                if method.default.is_default() {
                    quote! {
                        #arrow #ty
                    }
                } else {
                    quote! {
                        #arrow Option<#ty>
                    }
                }
            }
        };

        match method.default {
            DefaultValue::None => {
                quote! {
                    pub fn #ident(#inputs) #output {
                        match self {
                            #(#variants)*
                            _ => None,
                        }
                    }
                }
            }
            DefaultValue::Default => {
                quote! {
                    pub fn #ident(#inputs) #output {
                        match self {
                            #(#variants)*
                            _ => std::default::Default::default(),
                        }
                    }
                }
            }
            DefaultValue::Val(ref expr) => {
                quote! {
                    pub fn #ident(#inputs) #output {
                        match self {
                            #(#variants)*
                            _ => #expr,
                        }
                    }
                }
            }
        }
    }
}
