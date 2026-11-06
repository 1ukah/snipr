# Escrita da interface

Os textos do Snipr seguem um português técnico simplificado, inspirado no
ASD-STE100 (Simplified Technical English). Todo texto visível fica em
`ui/src/strings.ts`.

## Regras

1. Escreva frases curtas. Uma ideia por frase.
2. Use a voz ativa: "O Snipr salva a sessão", não "A sessão é salva".
3. Use sempre o mesmo termo para a mesma coisa: "busca", "loja", "login",
   "sessão", "API key", "agent".
4. Termos técnicos ficam em inglês quando é o uso comum: agent, login, token,
   API key, endpoint, headless.
5. Não use gírias nem formas coloquiais: "dá para", "pra", "debugar".
6. Instruções seguem a ordem das ações: "Faça o login na janela do Chrome.
   Depois, clique em Concluído."
7. Evite artigos antes de nomes de lojas ("na Amazon", "no Mercado Livre"):
   o gênero muda de loja para loja. Prefira frases neutras.
8. Texto só aparece quando o usuário precisa saber algo que não é óbvio.
   Ações óbvias usam ícone e rótulo curto.
