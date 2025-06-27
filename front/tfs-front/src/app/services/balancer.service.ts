import {Injectable} from '@angular/core';
import {HttpClient} from '@angular/common/http';
import {AuthService} from './auth.service';

@Injectable({
  providedIn: 'root'
})
export class BalancerService {

  // private url = 'http://127.0.0.1:8080/auth/api/';
  // private url = document.location.hostname + '/api/auth/api/';
  // private url = 'http://127.0.0.1:8080/virtual_fs';
  // private url = 'https://10.42.0.212:8081';
  private url = 'https://158.160.98.131:8081';

  private urls = [
    'https://158.160.98.131:8081',
    'https://158.160.186.112:8081',
    'https://158.160.21.119:8081',
  ]

  // private urls = [
    // 'https://10.42.0.212:8081',
    // 'https://10.42.0.1:8081'
  // ]

  constructor(private client: HttpClient) {
  }


  public getIP(): string {
    return this.urls[Math.floor(Math.random() * this.urls.length)];
  }
}
