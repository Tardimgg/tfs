import {UserTokens} from '../user_token';

export interface LoginResponse {
  uid: number,
  token: UserTokens;
}

